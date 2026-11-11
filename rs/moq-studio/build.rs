//! Rasterizes the app icon, `packaging/live.kixel.studio.svg`, into the RGBA
//! pixels the window's icon takes, so the SVG stays the only drawn copy.
//!
//! With `cef` on Linux, the binary loads `libcef.so` when it starts: from
//! beside itself, where a package installs the CEF runtime, or else from
//! `CEF_PATH` when the build has one, as the dev shell does.

use std::path::{Path, PathBuf};

/// The icon's edge in pixels: what a title bar, task switcher, or Dock draws
/// at most, scaled down from there.
const ICON_SIZE: u32 = 256;

fn main() {
	icon();

	println!("cargo::rerun-if-env-changed=CEF_PATH");
	let cef = std::env::var_os("CARGO_FEATURE_CEF").is_some();
	if !cef || std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
		return;
	}
	println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
	if let Some(dir) = std::env::var_os("CEF_PATH").filter(|dir| Path::new(dir).join("libcef.so").exists()) {
		println!("cargo::rustc-link-arg-bins=-Wl,-rpath,{}", Path::new(&dir).display());
	}
}

fn icon() {
	let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
	let source = manifest.join("../../packaging/live.kixel.studio.svg");
	println!("cargo::rerun-if-changed={}", source.display());

	let svg = std::fs::read(&source).unwrap_or_else(|err| panic!("cannot read {}: {err}", source.display()));
	let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).expect("the icon is not valid SVG");
	let mut pixmap = resvg::tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE).unwrap();
	let scale = ICON_SIZE as f32 / tree.size().width().max(tree.size().height());
	resvg::render(
		&tree,
		resvg::tiny_skia::Transform::from_scale(scale, scale),
		&mut pixmap.as_mut(),
	);

	// tiny-skia keeps premultiplied alpha; the window takes it straight.
	let rgba: Vec<u8> = pixmap
		.pixels()
		.iter()
		.flat_map(|pixel| {
			let color = pixel.demultiply();
			[color.red(), color.green(), color.blue(), color.alpha()]
		})
		.collect();

	let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
	std::fs::write(out.join("icon.rgba"), rgba).expect("cannot write the icon");
	println!("cargo::rustc-env=ICON_SIZE={ICON_SIZE}");
}
