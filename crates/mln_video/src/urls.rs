//! A video's URLs through the map's resource transform, for a player that
//! loads them itself.

use std::sync::Arc;

use mln_storage::resource::Kind;
use mln_storage::resource_transform::{ResourceTransform, TransformResult};
use parking_lot::Mutex;

/// What [`transform_urls`] answers: the URLs to play, in order, or why none
/// is left.
pub(crate) type Transformed = Result<Vec<String>, String>;

type Done = Box<dyn FnOnce(Transformed) + Send>;

/// Rewrites each of `urls` with `transform` as a [`Kind::Source`] resource,
/// as GL JS's `transformRequest(url, "Source")` does for a video's URLs,
/// and calls `done` once every answer came: with the rewritten URLs, in
/// order, less those the transform failed, or with why it failed them all.
/// Without a transform, or without URLs, `done` gets `urls` at once.
pub(crate) fn transform_urls(
    urls: Vec<String>,
    transform: Option<&ResourceTransform>,
    done: impl FnOnce(Transformed) + Send + 'static,
) {
    let Some(transform) = transform.filter(|_| !urls.is_empty()) else {
        done(Ok(urls));
        return;
    };
    let pending =
        Arc::new(Mutex::new(Pending { answers: urls.iter().map(|_| None).collect(), done: Some(Box::new(done)) }));
    for (index, url) in urls.iter().enumerate() {
        let pending = Arc::clone(&pending);
        transform(
            Kind::Source,
            url,
            Box::new(move |result| {
                let finished = {
                    let mut pending = pending.lock();
                    if let Some(answer) = pending.answers.get_mut(index) {
                        *answer = Some(result);
                    }
                    pending.finished()
                };
                if let Some((done, answers)) = finished {
                    done(kept(answers));
                }
            }),
        );
    }
}

struct Pending {
    /// Each URL's answer, once it came.
    answers: Vec<Option<TransformResult>>,
    done: Option<Done>,
}

impl Pending {
    /// What `done` gets, once every answer came, the first time.
    fn finished(&mut self) -> Option<(Done, Vec<TransformResult>)> {
        if self.answers.iter().any(Option::is_none) {
            return None;
        }
        let done = self.done.take()?;
        Some((done, self.answers.iter_mut().filter_map(Option::take).collect()))
    }
}

/// The URLs that `answers` kept, or the first failure when they kept none.
fn kept(answers: Vec<TransformResult>) -> Transformed {
    let mut urls = Vec::new();
    let mut failure = None;
    for answer in answers {
        match answer {
            Ok(url) => urls.push(url),
            Err(error) => {
                failure.get_or_insert(error.message);
            }
        }
    }
    match failure {
        Some(message) if urls.is_empty() => Err(format!("the resource transform failed its URLs: {message}")),
        _ => Ok(urls),
    }
}

#[cfg(test)]
mod tests {
    use mln_storage::ResourceError;
    use mln_storage::response::Reason;

    use super::*;

    fn run(urls: &[&str], transform: Option<&ResourceTransform>) -> Option<Transformed> {
        let answer = Arc::new(Mutex::new(None));
        let set = Arc::clone(&answer);
        transform_urls(urls.iter().map(|&url| url.to_owned()).collect(), transform, move |urls| {
            *set.lock() = Some(urls);
        });
        answer.lock().take()
    }

    #[test]
    fn without_a_transform_the_urls_stay() {
        assert_eq!(Some(Ok(vec!["a.webm".to_owned()])), run(&["a.webm"], None));
    }

    #[test]
    fn the_transform_rewrites_each_url_as_a_source_and_drops_those_it_fails() {
        let transform: ResourceTransform = Arc::new(|kind, url, done| {
            assert_eq!(Kind::Source, kind);
            if url.starts_with("bad") {
                done(Err(ResourceError::new(Reason::Other, "refused")));
            } else {
                done(Ok(format!("https://example.com/{url}")));
            }
        });
        assert_eq!(
            Some(Ok(vec!["https://example.com/a.mp4".to_owned(), "https://example.com/b.webm".to_owned()])),
            run(&["a.mp4", "bad.ogv", "b.webm"], Some(&transform)),
        );
        assert_eq!(
            Some(Err("the resource transform failed its URLs: refused".to_owned())),
            run(&["bad.mp4"], Some(&transform)),
        );
    }

    #[test]
    fn the_urls_come_once_every_answer_did() {
        let answers = Arc::new(Mutex::new(Vec::new()));
        let held = Arc::clone(&answers);
        let transform: ResourceTransform = Arc::new(move |_, url, done| held.lock().push((url.to_owned(), done)));
        let result = Arc::new(Mutex::new(None));
        let set = Arc::clone(&result);
        transform_urls(vec!["a".to_owned(), "b".to_owned()], Some(&transform), move |urls| {
            *set.lock() = Some(urls);
        });
        let mut pending = std::mem::take(&mut *answers.lock());
        let (url, done) = pending.pop().unwrap();
        done(Ok(format!("{url}2")));
        assert!(result.lock().is_none());
        let (url, done) = pending.pop().unwrap();
        done(Ok(format!("{url}1")));
        assert_eq!(Some(Ok(vec!["a1".to_owned(), "b2".to_owned()])), result.lock().take());
    }
}
