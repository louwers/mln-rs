//! The handles of the style's sources, which load their descriptions and
//! change their records as they do. The style holds only the records
//! ([`mln_style::SourceList`]); [`StyleSources::sync`] writes a handle's
//! current record into the style.

use std::rc::Rc;
use std::sync::Arc;

use mln_actor::RunLoop;
use mln_storage::FileSource;
use mln_style::source::{self, Source, SourceObserver};
use mln_style::{Edit, Style};
use mln_util::error::Error;
use mln_util::event_queue::EventQueue;

/// What a source reported about itself.
pub(crate) enum SourceEvent {
    /// The source changed its record, as a setter does.
    Changed(String),
    /// The source loaded its description.
    Loaded,
    /// The source reloads its description, and isn't loaded meanwhile.
    DescriptionChanged,
    /// The source failed to load.
    Error(String, Error),
}

/// The sources' observer: queues their reports for the map, and starts the
/// description load of a source whose description changed.
struct SourceEvents {
    events: EventQueue<SourceEvent>,
    file_source: Option<Arc<dyn FileSource>>,
    run_loop: RunLoop,
}

impl SourceObserver for SourceEvents {
    fn on_source_loaded(&self, _source: &dyn Source) {
        self.events.push(SourceEvent::Loaded);
    }

    fn on_source_changed(&self, source: &dyn Source) {
        self.events.push(SourceEvent::Changed(source.get_id()));
    }

    fn on_source_error(&self, source: &dyn Source, error: Error) {
        self.events.push(SourceEvent::Error(source.get_id(), error));
    }

    fn on_source_description_changed(&self, source: &dyn Source) {
        self.events.push(SourceEvent::DescriptionChanged);
        if !source.loaded()
            && let Some(file_source) = &self.file_source
        {
            source.load_description(&**file_source, &self.run_loop);
        }
    }
}

/// The handles of the style's sources, in the order they were added.
pub(crate) struct StyleSources {
    sources: Vec<(Arc<str>, Box<dyn Source>)>,
    observer: Rc<SourceEvents>,
}

impl StyleSources {
    /// Sources that load their descriptions from `file_source`, answering
    /// on `run_loop`, and report to `events`.
    pub(crate) fn new(
        file_source: Option<Arc<dyn FileSource>>,
        run_loop: &RunLoop,
        events: EventQueue<SourceEvent>,
    ) -> Self {
        StyleSources {
            sources: Vec::new(),
            observer: Rc::new(SourceEvents { events, file_source, run_loop: run_loop.clone() }),
        }
    }

    pub(crate) fn get(&self, id: &str) -> Option<&dyn Source> {
        self.sources.iter().find(|(key, _)| **key == *id).map(|(_, source)| source.as_ref())
    }

    /// The sources, in the order they were added.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &dyn Source> {
        self.sources.iter().map(|(_, source)| source.as_ref())
    }

    /// Adds `source` last and starts loading its description, and returns
    /// its id and record. Fails with
    /// [`ErrorKind::AlreadyExists`](mln_util::ErrorKind) if a source has its
    /// id.
    pub(crate) fn add(&mut self, source: Box<dyn Source>) -> Result<(String, Arc<dyn source::Impl>), Error> {
        let id = source.get_id();
        if self.get(&id).is_some() {
            return Err(Error::already_exists(format!("Source {id} already exists")));
        }
        let observer: Rc<dyn SourceObserver> = Rc::<SourceEvents>::clone(&self.observer);
        source.set_observer(Some(observer));
        if let Some(file_source) = &self.observer.file_source {
            source.load_description(&**file_source, &self.observer.run_loop);
        }
        let record = source.base_impl();
        self.sources.push((Arc::from(id.as_str()), source));
        Ok((id, record))
    }

    /// Removes the source `id`, which stops reporting.
    pub(crate) fn remove(&mut self, id: &str) -> Option<Box<dyn Source>> {
        let index = self.sources.iter().position(|(key, _)| **key == *id)?;
        let (_, source) = self.sources.remove(index);
        source.set_observer(None);
        Some(source)
    }

    /// Replaces every source with `sources`, a parsed style's, and starts
    /// loading their descriptions.
    pub(crate) fn replace_all(&mut self, sources: Vec<Box<dyn Source>>) {
        for (_, source) in self.sources.drain(..) {
            source.set_observer(None);
        }
        for source in sources {
            // A parsed style's sources have unique ids.
            let _ = self.add(source);
        }
    }

    /// Writes into `style` the record of each source whose record changed
    /// since `style` last took it, copying the version only then. Returns
    /// whether any did.
    pub(crate) fn sync(&self, style: &mut Arc<Style>) -> bool {
        let stale = |style: &Style, id: &str, source: &dyn Source| {
            !style.get_source(id).is_some_and(|record| mln_style::Source::ptr_eq(record, &source.base_impl()))
        };
        if !self.sources.iter().any(|(id, source)| stale(style, id, &**source)) {
            return false;
        }
        let style = Arc::make_mut(style);
        for (id, source) in &self.sources {
            if stale(style, id, &**source) {
                // Recording a source's record can't fail.
                let _ = style.apply(Edit::SetSourceImpl { id: id.to_string(), source: Some(source.base_impl()) });
            }
        }
        true
    }

    /// Whether every source loaded its description.
    pub(crate) fn all_loaded(&self) -> bool {
        self.sources.iter().all(|(_, source)| source.loaded())
    }

    /// Logs the sources and their state.
    pub(crate) fn dump_debug_logs(&self) {
        for (_, source) in &self.sources {
            source.dump_debug_logs();
        }
    }
}
