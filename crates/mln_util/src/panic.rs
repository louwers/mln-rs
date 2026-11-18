//! Panics that the library survives. A panic is a bug in the library: the
//! work that hit it fails, with the panic's message, instead of the process.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Runs `work`, and returns the message of the panic that ended it instead
/// of its result. The panic hook has reported the panic already.
///
/// The state that `work` changed may be half-changed after a panic, so the
/// caller must not rely on it, such as by failing what owns it.
///
/// # Errors
///
/// Returns the panic's message when `work` panicked.
pub fn catch<R>(work: impl FnOnce() -> R) -> Result<R, String> {
    catch_unwind(AssertUnwindSafe(work)).map_err(|payload| message(&*payload))
}

/// The message of a panic whose payload is `payload`.
#[must_use]
pub fn message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "a panic without a message".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_result_of_work_that_returns() {
        assert_eq!(catch(|| 7), Ok(7));
    }

    #[test]
    fn returns_the_message_of_a_panic() {
        assert_eq!(catch(|| panic!("static")), Err::<(), _>("static".to_owned()));
        let value = 7;
        assert_eq!(catch(|| panic!("formatted {value}")), Err::<(), _>("formatted 7".to_owned()));
        assert_eq!(catch(|| std::panic::panic_any(7)), Err::<(), _>("a panic without a message".to_owned()));
    }
}
