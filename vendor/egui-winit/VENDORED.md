# egui-winit source

Copied from the crates.io release of `egui-winit` 0.37.0 on 30 October 2026, with the licence
files of `emilk/egui` at tag `0.37.0`. Licensed under MIT OR Apache-2.0.

One change, marked `TracePlotter:` in `src/lib.rs`: on Windows the window is created with
`WS_EX_NOREDIRECTIONBITMAP`, through winit's `with_no_redirection_bitmap`. eframe draws through
DirectComposition, which Glass's transparency needs, so the GDI surface Windows otherwise gives
the window is never painted. Screenshot tools still blend it in, which covered the window, or
the part of it as large as it was when it opened, with a grey veil. The style can only be set
when the window is created, and egui-winit has no option for it.

`Cargo.toml` patches crates.io's egui-winit with this directory. When egui is updated, replace
this directory with the new release, apply the change again, and check a screenshot of the
Glass theme on Windows 11. Drop the copy once egui offers the setting itself.
