# Stream A hand-over (Rust core)
- core/ = iloader-core. Root Cargo.toml workspace = core + src-tauri (add android/bridge later). Cargo.lock moved to repo root; target/ is now at repo root.
- Contract additions: Host::cache_dir() (default data_dir/cache) and Host::kv() (default: JSON file data_dir/data.json) are default methods; SecretStore = get/set/delete(-> bool existed)/get_bytes/set_bytes (flat keys, same names as the desktop keyring: email, "anisette_state", isideload keys).
- attach_usb_device is async, max_packet_out is u32; fd is dup'ed internally (Kotlin keeps its UsbDeviceConnection). detach_usb_device(name) is sync.
- Core::new on Android starts its own tokio runtime (2 threads), binds data_dir/usbmuxd.sock unless a muxer is already registered (Tauri plugin).
- Core::new installs the global tracing subscriber (file logs in data_dir/logs + CoreEvent::Log) once; panic hook too.
- Core has no `Core::login*` &Arc requirement: methods take &self (Weak self inside).
