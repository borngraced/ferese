use cosmic::iced::futures::SinkExt;
use ferese_config::theme::Snapshot;

pub fn current() -> Snapshot {
    ferese_ipc::theme::current()
}

pub fn opacity(theme: &ferese_config::theme::ResolvedTheme) -> f32 {
    if theme.tokens.material.style == "translucent" {
        theme.tokens.material.opacity as f32
    } else {
        1.
    }
}

pub fn subscription() -> cosmic::iced::Subscription<Snapshot> {
    cosmic::iced::Subscription::run(stream)
}

fn stream() -> impl cosmic::iced::futures::Stream<Item = Snapshot> {
    cosmic::iced::stream::channel(1, async |mut output| {
        loop {
            let connection = tokio::task::spawn_blocking(ferese_ipc::theme::Connection::connect).await;
            let Ok(Ok(mut connection)) = connection else {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            };
            let Ok(cancellation) = connection.cancellation() else {
                return;
            };
            let (send, mut receive) = tokio::sync::watch::channel(None);
            tokio::task::spawn_blocking(move || {
                let Ok(mut snapshot) = connection.get() else { return };
                loop {
                    let revision = snapshot.revision;
                    if send.send(Some(snapshot)).is_err() {
                        return;
                    }
                    match connection.watch(revision) {
                        Ok(next) => snapshot = next,
                        Err(_) => return,
                    }
                }
            });
            while receive.changed().await.is_ok() {
                let snapshot = receive.borrow_and_update().clone();
                if let Some(snapshot) = snapshot {
                    if output.send(snapshot).await.is_err() {
                        return;
                    }
                }
            }
            drop(cancellation);
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    })
}
