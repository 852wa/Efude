// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Gives the Windows executable its icon (shown by Explorer, the taskbar
//! and the installer).

fn main() {
    println!("cargo:rerun-if-changed=../../assets/efude.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/efude.ico");
        if let Err(error) = resource.compile() {
            panic!("could not embed the Windows icon: {error}");
        }
    }
}
