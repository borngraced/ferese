//! Generated bindings for Ferese-private Wayland protocols.

#![forbid(unsafe_code)]

/// Semantic surface effects protocol.
pub mod effects {
    /// Version 1 of the semantic surface effects protocol.
    pub mod v1 {
        #[cfg(feature = "client")]
        pub use generated::client;
        #[cfg(feature = "server")]
        pub use generated::server;

        mod generated {
            #![allow(
                dead_code,
                non_camel_case_types,
                non_upper_case_globals,
                non_snake_case,
                unused_imports,
                unused_unsafe,
                unused_variables,
                clippy::all
            )]

            #[cfg(feature = "client")]
            pub mod client {
                use wayland_client;
                use wayland_client::protocol::*;

                pub mod __interfaces {
                    use wayland_client::protocol::__interfaces::*;

                    wayland_scanner::generate_interfaces!("../../protocols/ferese-effects-v1.xml");
                }
                use self::__interfaces::*;

                wayland_scanner::generate_client_code!("../../protocols/ferese-effects-v1.xml");
            }

            #[cfg(feature = "server")]
            pub mod server {
                use wayland_server;
                use wayland_server::protocol::*;

                pub mod __interfaces {
                    use wayland_server::protocol::__interfaces::*;

                    wayland_scanner::generate_interfaces!("../../protocols/ferese-effects-v1.xml");
                }
                use self::__interfaces::*;

                wayland_scanner::generate_server_code!("../../protocols/ferese-effects-v1.xml");
            }
        }
    }
}

/// Private shell control and managed-window metadata protocol.
pub mod shell {
    /// Version 1 of the Ferese shell control protocol.
    pub mod v1 {
        #[cfg(feature = "client")]
        pub use generated::client;
        #[cfg(feature = "server")]
        pub use generated::server;

        mod generated {
            #![allow(
                dead_code,
                non_camel_case_types,
                non_upper_case_globals,
                non_snake_case,
                unused_imports,
                unused_unsafe,
                unused_variables,
                clippy::all
            )]

            #[cfg(feature = "client")]
            pub mod client {
                use wayland_client;
                use wayland_client::protocol::*;

                pub mod __interfaces {
                    use wayland_client::protocol::__interfaces::*;

                    wayland_scanner::generate_interfaces!("../../protocols/ferese-shell-v1.xml");
                }
                use self::__interfaces::*;

                wayland_scanner::generate_client_code!("../../protocols/ferese-shell-v1.xml");
            }

            #[cfg(feature = "server")]
            pub mod server {
                use wayland_server;
                use wayland_server::protocol::*;

                pub mod __interfaces {
                    use wayland_server::protocol::__interfaces::*;

                    wayland_scanner::generate_interfaces!("../../protocols/ferese-shell-v1.xml");
                }
                use self::__interfaces::*;

                wayland_scanner::generate_server_code!("../../protocols/ferese-shell-v1.xml");
            }
        }
    }
}
