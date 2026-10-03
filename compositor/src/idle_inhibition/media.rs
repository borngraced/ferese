//! MPRIS reads and signal handling run off the compositor thread, without polling.
use std::collections::HashMap;
use std::time::Duration;

use calloop::EventLoop;
use calloop::channel::{Event, channel};
use futures_lite::{StreamExt, future, stream};
use zbus::zvariant::OwnedValue;
use zbus::{Connection, MatchRule, MessageStream};

use crate::Ferese;

const PREFIX: &str = "org.mpris.MediaPlayer2";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const MAX_PLAYERS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Player {
    pub name: String,
    pub owner: String,
    pub pid: Option<u32>,
    pub desktop_entry: Option<String>,
    pub playing: bool,
}

async fn bus_call<T: serde::de::DeserializeOwned + zbus::zvariant::Type>(
    connection: &Connection,
    member: &str,
    name: &str,
) -> zbus::Result<T> {
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            member,
            &(name,),
        )
        .await?
        .body()
        .deserialize()
}

async fn property(connection: &Connection, owner: &str, interface: &str, name: &str) -> zbus::Result<OwnedValue> {
    connection
        .call_method(Some(owner), PATH, Some(PROPERTIES), "Get", &(interface, name))
        .await?
        .body()
        .deserialize()
}

async fn refresh_player(connection: &Connection, player: &mut Player, identity: bool) {
    // Bind reads to the unique owner, never a name that can change mid-query.
    player.playing = property(connection, &player.owner, PLAYER, "PlaybackStatus")
        .await
        .ok()
        .and_then(|value| String::try_from(value).ok())
        .is_some_and(|status| status == "Playing");

    if identity {
        player.pid = bus_call(connection, "GetConnectionUnixProcessID", &player.owner)
            .await
            .ok();
        player.desktop_entry = property(connection, &player.owner, PREFIX, "DesktopEntry")
            .await
            .ok()
            .and_then(|value| String::try_from(value).ok())
            .map(|value| crate::window_rules::normalize_app_id(&value))
            .filter(|value| !value.is_empty());
    }
}

fn publish(
    players: &HashMap<String, Player>,
    previous: &mut Vec<Player>,
    send: &mut impl FnMut(Vec<Player>) -> bool,
) -> bool {
    let mut snapshot: Vec<_> = players.values().cloned().collect();
    snapshot.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    if snapshot == *previous {
        return true;
    }

    *previous = snapshot.clone();
    send(snapshot)
}

async fn monitor(connection: Connection, mut send: impl FnMut(Vec<Player>) -> bool) -> zbus::Result<()> {
    // Subscribe before discovery so startup and owner replacement cannot lose
    // a pause/stop event. Ignore unrelated property traffic such as position.
    let properties = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(PATH)?
        .interface(PROPERTIES)?
        .member("PropertiesChanged")?
        .build();
    let owners = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .arg0ns(PREFIX)?
        .build();
    let properties = MessageStream::for_match_rule(properties, &connection, Some(128)).await?;
    let owners = MessageStream::for_match_rule(owners, &connection, Some(128)).await?;
    let mut signals = stream::or(properties, owners);
    let mut players = HashMap::new();
    let mut previous = Vec::new();
    let names: Vec<String> = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )
        .await?
        .body()
        .deserialize()?;
    let mut names: Vec<_> = names
        .into_iter()
        .filter(|name| name.strip_prefix(PREFIX).is_some_and(|suffix| suffix.starts_with('.')))
        .collect();
    names.sort_unstable();
    for name in names.into_iter().take(MAX_PLAYERS) {
        if let Ok(owner) = bus_call(&connection, "GetNameOwner", &name).await {
            let mut player = Player {
                name: name.clone(),
                owner,
                pid: None,
                desktop_entry: None,
                playing: false,
            };
            refresh_player(&connection, &mut player, true).await;
            players.insert(name, player);
        }
    }

    if !publish(&players, &mut previous, &mut send) {
        return Ok(());
    }
    while let Some(message) = signals.next().await {
        let message = message?;
        let header = message.header();
        if header
            .member()
            .is_some_and(|member| member.as_str() == "NameOwnerChanged")
        {
            let (name, _, owner): (String, String, String) = message.body().deserialize()?;
            // Remove the old owner's state before looking up the replacement.
            players.remove(&name);
            if !publish(&players, &mut previous, &mut send) {
                return Ok(());
            }

            if !owner.is_empty() && players.len() < MAX_PLAYERS {
                let mut player = Player {
                    name: name.clone(),
                    owner,
                    pid: None,
                    desktop_entry: None,
                    playing: false,
                };
                refresh_player(&connection, &mut player, true).await;
                players.insert(name, player);
            }
        } else {
            let Ok((interface, changed, invalidated)) =
                message
                    .body()
                    .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
            else {
                continue;
            };
            let identity = interface == PREFIX
                && (changed.contains_key("DesktopEntry") || invalidated.iter().any(|field| field == "DesktopEntry"));
            let status = interface == PLAYER
                && (changed.contains_key("PlaybackStatus")
                    || invalidated.iter().any(|field| field == "PlaybackStatus"));
            if !identity && !status {
                continue;
            }

            let Some(owner) = header.sender() else {
                continue;
            };
            for player in players.values_mut().filter(|player| player.owner == owner.as_str()) {
                // Re-read current state: a signal queued during discovery may
                // describe older playback than the startup snapshot.
                refresh_player(&connection, player, identity).await;
            }
        }

        if !publish(&players, &mut previous, &mut send) {
            return Ok(());
        }
    }

    Err(zbus::Error::Failure("MPRIS signal connection ended".into()))
}

pub(crate) fn init(event_loop: &mut EventLoop<Ferese>) -> Result<(), Box<dyn std::error::Error>> {
    let (sender, receiver) = channel();
    event_loop.handle().insert_source(receiver, |event, _, state| {
        if let Event::Msg(players) = event {
            state.media_players = players;
            state.refresh_idle_inhibition();
        }
    })?;

    std::thread::Builder::new()
        .name("ferese-playback".into())
        .spawn(move || {
            let mut had_players = false;
            loop {
                let result = future::block_on(async {
                    let connection = zbus::connection::Builder::session()?
                        .method_timeout(Duration::from_millis(500))
                        .build()
                        .await?;
                    monitor(connection, |players| {
                        had_players = !players.is_empty();
                        sender.send(players).is_ok()
                    })
                    .await
                });
                match result {
                    Ok(()) => break,
                    Err(error) => {
                        tracing::debug!(%error, "playback monitor disconnected; releasing automatic inhibition")
                    }
                }

                if had_players && sender.send(Vec::new()).is_err() {
                    break;
                }
                had_players = false;
                // Reconnect only after failure; healthy operation sleeps on signals.
                std::thread::sleep(Duration::from_secs(5));
            }
        })?;
    Ok(())
}
