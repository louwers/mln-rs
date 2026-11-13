//! `web.h`: the JavaScript objects that a browser host lends the library, as
//! handles into a table of the instance's, and linear memory for the host's
//! descriptors.
//!
//! The browser build's other JavaScript-facing exports, which wasm-bindgen
//! makes rather than the headers, live here too: the table's `insert` and
//! `get`, the callback trampolines (`trampoline.rs`), and the defaults of the
//! instance's tile workers and shaper module, which the package ships beside
//! the module's bindings.

#[cfg(target_family = "wasm")]
mod table {
    use std::cell::RefCell;

    use wasm_bindgen::prelude::*;

    use crate::error::Error;

    /// The objects, by handle; handle `index + 1` is at `index`.
    struct Table {
        objects: Vec<Option<JsValue>>,
        free: Vec<u32>,
    }

    thread_local! {
        static TABLE: RefCell<Table> = const { RefCell::new(Table { objects: Vec::new(), free: Vec::new() }) };
    }

    /// Puts `value` in the table, and returns its handle: an
    /// `mln_web_object`, which `mln_web_object_release()` releases.
    #[expect(unreachable_pub, reason = "wasm-bindgen exports only `pub` functions")]
    #[wasm_bindgen(js_name = mlnWebObjectInsert)]
    #[must_use]
    pub fn insert(value: JsValue) -> u32 {
        TABLE.with(|table| {
            let mut table = table.borrow_mut();
            if let Some(handle) = table.free.pop() {
                table.objects[handle as usize - 1] = Some(value);
                handle
            } else {
                table.objects.push(Some(value));
                table.objects.len() as u32
            }
        })
    }

    /// The object of a handle, or `undefined` for one that names none.
    #[expect(unreachable_pub, reason = "wasm-bindgen exports only `pub` functions")]
    #[wasm_bindgen(js_name = mlnWebObjectGet)]
    #[must_use]
    pub fn get(handle: u32) -> JsValue {
        object(handle).unwrap_or(JsValue::UNDEFINED)
    }

    /// The object of `handle`, if it names one.
    pub(crate) fn object(handle: u32) -> Option<JsValue> {
        let index = (handle as usize).checked_sub(1)?;
        TABLE.with(|table| table.borrow().objects.get(index).cloned().flatten())
    }

    /// The object of a descriptor's `name`d handle, which must name one.
    pub(crate) fn required(handle: u32, name: &str) -> Result<JsValue, Error> {
        if handle == 0 {
            return Err(Error::invalid_argument(format!("{name} must not be 0")));
        }
        object(handle).ok_or_else(|| Error::invalid_argument(format!("{name} names no object")))
    }

    pub(super) fn release(handle: u32) {
        let Some(index) = (handle as usize).checked_sub(1) else { return };
        TABLE.with(|table| {
            let mut table = table.borrow_mut();
            if table.objects.get_mut(index).and_then(Option::take).is_some() {
                table.free.push(handle);
            }
        });
    }
}

#[cfg(target_family = "wasm")]
pub(crate) use table::required;

/// The callback trampolines: the browser host can't put its own functions
/// in the module's function table, so it puts a trampoline's address in a
/// callback field (`mlnWebUpcallPointer`), and its own key in the user data.
/// The trampoline calls the host's dispatcher (`mlnWebSetUpcallDispatcher`)
/// with the callback typedef's name and its arguments: integers of up to 32
/// bits, floats and addresses as numbers, 64-bit integers as `BigInt`s, and
/// a struct passed by value as the address of a copy that lives until the
/// dispatcher returns. The dispatcher's result is the callback's.
#[cfg(target_family = "wasm")]
mod upcalls {
    use std::cell::RefCell;

    use mln_util::logging::{Event, Log};
    use wasm_bindgen::prelude::*;

    thread_local! {
        static DISPATCHER: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
    }

    /// Sets the function that the trampolines call: `dispatcher(name,
    /// ...arguments)`.
    #[expect(unreachable_pub, reason = "wasm-bindgen exports only `pub` functions")]
    #[wasm_bindgen(js_name = mlnWebSetUpcallDispatcher)]
    pub fn set_dispatcher(dispatcher: js_sys::Function) {
        DISPATCHER.with(|cell| *cell.borrow_mut() = Some(dispatcher));
    }

    /// The address of the trampoline of the callback typedef `name`, such as
    /// `"mln_wake_callback"`, for a callback field, or 0 for a name that the
    /// headers don't declare.
    #[expect(unreachable_pub, reason = "wasm-bindgen exports only `pub` functions")]
    #[wasm_bindgen(js_name = mlnWebUpcallPointer)]
    #[must_use]
    pub fn pointer(name: &str) -> u32 {
        crate::generated::trampolines::pointer(name).map_or(0, |pointer| pointer as u32)
    }

    /// Calls the host's dispatcher. A dispatcher that throws, or none, gives
    /// `undefined`, which a trampoline returns as zero.
    pub(crate) fn dispatch(name: &str, arguments: &[JsValue]) -> JsValue {
        let Some(dispatcher) = DISPATCHER.with(|cell| cell.borrow().clone()) else {
            Log::error(Event::General, format!("{name} ran with no upcall dispatcher set"));
            return JsValue::UNDEFINED;
        };
        let call = std::iter::once(JsValue::from_str(name)).chain(arguments.iter().cloned()).collect::<js_sys::Array>();
        dispatcher.apply(&JsValue::NULL, &call).unwrap_or_else(|error| {
            Log::error(Event::General, format!("the upcall dispatcher threw for {name}: {error:?}"));
            JsValue::UNDEFINED
        })
    }
}

use core::ffi::c_void;
use std::alloc::Layout;

#[cfg(target_family = "wasm")]
pub(crate) use upcalls::dispatch;

use crate::generated::types::mln_web_object;

/// `mln_web_object_release()`. The other builds have no objects to release.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn mln_web_object_release(object: mln_web_object) {
    #[cfg(target_family = "wasm")]
    table::release(object);
    #[cfg(not(target_family = "wasm"))]
    let _ = object;
}

/// `mln_web_alloc()`.
#[unsafe(no_mangle)]
pub(crate) extern "C" fn mln_web_alloc(size: usize, align: usize) -> *mut c_void {
    match Layout::from_size_align(size, align) {
        // SAFETY: the layout has a size.
        Ok(layout) if size != 0 => unsafe { std::alloc::alloc(layout) }.cast(),
        _ => core::ptr::null_mut(),
    }
}

/// `mln_web_free()`.
///
/// # Safety
///
/// `pointer` is null or what `mln_web_alloc(size, align)` returned, not freed
/// yet.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mln_web_free(pointer: *mut c_void, size: usize, align: usize) {
    if pointer.is_null() {
        return;
    }
    if let Ok(layout) = Layout::from_size_align(size, align) {
        // SAFETY: the caller's contract.
        unsafe { std::alloc::dealloc(pointer.cast(), layout) }
    }
}

/// Sets the defaults of the instance's tile workers and shaper module: the
/// files that the package ships beside the module's bindings, which a host
/// replaces with `mlnWebConfigureTileWorkers` and `mlnWebSetShaperLoader`.
#[cfg(target_family = "wasm")]
pub(crate) fn set_defaults() {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "/js/package.js")]
    extern "C" {
        #[wasm_bindgen(js_name = shaperLoader)]
        fn shaper_loader() -> js_sys::Function;
    }

    thread_local! {
        static SET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if SET.replace(true) {
        return;
    }
    mln_web_host::tile_workers::set_default_script(wasm_bindgen::link_to!(module = "/js/tile_worker.js"));
    mln_web_host::shaper::set_default_loader(shaper_loader());
}
