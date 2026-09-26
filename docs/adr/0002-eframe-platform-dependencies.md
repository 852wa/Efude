# ADR 0002: Keep eframe's platform dependencies within the project policy

## Status

Accepted

## Context

The selected eframe 0.31 release unconditionally enables egui-winit's OS
clipboard feature. That feature pulls `arboard`, whose Windows backend depends
on `clipboard-win` and `error-code` under BSL-1.0. The project dependency
policy does not allow BSL-1.0. The same release's default font feature bundles
fonts under OFL-1.1 and Ubuntu Font License, while Efude uses installed system
fonts and does not redistribute font files.

## Decision

Vendor the upstream eframe 0.31.1 crate and keep its changes limited to its
Cargo feature declarations: remove the mandatory clipboard feature from the
egui-winit dependency and remove the default-font feature from eframe. The app
loads fonts from the operating system. Windows image clipboard transfer uses
the Win32 DIB/DIBV5 formats through the existing `windows` dependency. Text
clipboard remains egui's in-process fallback when the OS clipboard feature is
disabled.

The vendored package retains the upstream Apache-2.0 and MIT license texts and
is marked non-publishable. When changing eframe versions, re-check its
clipboard and font feature graph before replacing the vendored copy.

## Consequences

- No BSL-1.0 clipboard crates or bundled font packages are in the active app
  dependency graph.
- Windows can copy transparent images to, and paste DIB/DIBV5 images from, the
  OS clipboard.
- The vendored eframe source requires deliberate updates when upgrading the
  UI framework.
