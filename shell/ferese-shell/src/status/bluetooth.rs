//! BlueZ status from its object graph; never starts discovery or pairing.
use super::Bluetooth;
use zbus::{blocking::Connection, fdo::ManagedObjects};

pub(super) fn read(connection: &Connection) -> zbus::Result<Option<Bluetooth>> {
    let objects: ManagedObjects = connection
        .call_method(
            Some("org.bluez"),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )?
        .body()
        .deserialize()?;
    Ok(snapshot(&objects))
}

fn snapshot(objects: &ManagedObjects) -> Option<Bluetooth> {
    // bluetoothctl's fresh process uses its first controller. Prefer hci0 and
    // keep the selection stable when the object-manager dictionary is reordered.
    let (adapter, properties) = objects
        .iter()
        .filter_map(|(path, interfaces)| interfaces.get("org.bluez.Adapter1").map(|p| (path, p)))
        .min_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()))?;
    let enabled = bool::try_from(properties.get("Powered")?).ok()?;
    let mut devices = Vec::new();
    if enabled {
        for interfaces in objects.values() {
            let Some(device) = interfaces.get("org.bluez.Device1") else {
                continue;
            };
            let owner = <&zbus::zvariant::ObjectPath>::try_from(device.get("Adapter")?).ok()?;
            if owner.as_str() != adapter.as_str() {
                continue;
            }
            if bool::try_from(device.get("Connected")?).ok()? {
                let alias = <&str>::try_from(device.get("Alias")?).ok()?;
                devices.push(alias.to_owned());
            }
        }
    }
    devices.sort();
    Some(Bluetooth { enabled, devices })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Str};
    fn graph() -> ManagedObjects {
        let mut graph = ManagedObjects::new();
        for (path, powered) in [("/org/bluez/hci0", true), ("/org/bluez/hci1", false)] {
            graph.insert(
                OwnedObjectPath::try_from(path).unwrap(),
                [(
                    "org.bluez.Adapter1".try_into().unwrap(),
                    [("Powered".into(), OwnedValue::from(powered))].into(),
                )]
                .into(),
            );
        }
        for (path, adapter, connected, alias) in [
            (
                "/org/bluez/hci0/dev_a",
                "/org/bluez/hci0",
                true,
                "Headphones",
            ),
            (
                "/org/bluez/hci0/dev_b",
                "/org/bluez/hci0",
                false,
                "Keyboard",
            ),
            (
                "/org/bluez/hci1/dev_a",
                "/org/bluez/hci1",
                true,
                "Other adapter",
            ),
        ] {
            graph.insert(
                OwnedObjectPath::try_from(path).unwrap(),
                [(
                    "org.bluez.Device1".try_into().unwrap(),
                    [
                        (
                            "Adapter".into(),
                            OwnedValue::try_from(zbus::zvariant::Value::from(
                                OwnedObjectPath::try_from(adapter).unwrap(),
                            ))
                            .unwrap(),
                        ),
                        ("Connected".into(), OwnedValue::from(connected)),
                        ("Alias".into(), OwnedValue::from(Str::from(alias))),
                    ]
                    .into(),
                )]
                .into(),
            );
        }
        graph
    }
    #[test]
    fn selects_one_adapter_and_only_its_connected_devices() {
        let mut objects = graph();
        let value = snapshot(&objects).unwrap();
        assert!(value.enabled);
        assert_eq!(value.devices, ["Headphones"]);
        let adapter = objects
            .get_mut(&OwnedObjectPath::try_from("/org/bluez/hci0").unwrap())
            .unwrap()
            .get_mut("org.bluez.Adapter1")
            .unwrap();
        adapter.insert("Powered".into(), false.into());
        let value = snapshot(&objects).unwrap();
        assert!(!value.enabled && value.devices.is_empty());
        adapter_removed(&mut objects);
        assert!(!snapshot(&objects).unwrap().enabled);
        assert!(snapshot(&ManagedObjects::new()).is_none());
    }
    fn adapter_removed(objects: &mut ManagedObjects) {
        objects.retain(|path, _| !path.as_str().starts_with("/org/bluez/hci0"));
    }
    #[test]
    fn incomplete_adapter_does_not_invent_power_state() {
        let mut objects = graph();
        for interfaces in objects.values_mut() {
            if let Some(adapter) = interfaces.get_mut("org.bluez.Adapter1") {
                adapter.remove("Powered");
            }
        }
        assert!(snapshot(&objects).is_none());
    }
    struct Manager;
    #[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
    impl Manager {
        fn get_managed_objects(&self) -> ManagedObjects {
            graph()
        }
    }
    #[test]
    #[ignore = "requires dbus-daemon; uses a private test bus"]
    fn bluetooth_bus_roundtrip_and_service_recovery() {
        let bus = crate::status::tests::TestBus::new();
        let server = zbus::blocking::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.bluez")
            .unwrap()
            .serve_at("/", Manager)
            .unwrap()
            .build()
            .unwrap();
        let client = bus.connect();
        assert_eq!(read(&client).unwrap().unwrap().devices, ["Headphones"]);
        server.release_name("org.bluez").unwrap();
        assert!(read(&client).is_err());
        server.request_name("org.bluez").unwrap();
        assert!(read(&client).unwrap().unwrap().enabled);
    }
}
