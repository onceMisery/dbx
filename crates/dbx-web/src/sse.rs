use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use tokio::sync::broadcast;
use tokio::sync::watch;

pub fn sse_from_channel(
    mut rx: broadcast::Receiver<String>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let stream = async_stream::stream! {
        while let Ok(data) = rx.recv().await {
            yield Ok(Event::default().data(data));
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub fn sse_from_watch(
    mut rx: watch::Receiver<String>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let stream = async_stream::stream! {
        let initial = rx.borrow().clone();
        if !initial.is_empty() {
            yield Ok(Event::default().data(initial));
        }
        while rx.changed().await.is_ok() {
            let update = rx.borrow().clone();
            yield Ok(Event::default().data(update));
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}
