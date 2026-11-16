//! Rasterizes the app icon, `packaging/live.kixel.studio.svg`, into the RGBA
//! pixels the window's icon takes, and for a macOS target into the `.icns`
//! the app bundle names, so the SVG stays the only drawn copy.
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
	let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());

	// tiny-skia keeps premultiplied alpha; the window takes it straight.
	let rgba: Vec<u8> = render(&tree, ICON_SIZE)
		.pixels()
		.iter()
		.flat_map(|pixel| {
			let color = pixel.demultiply();
			[color.red(), color.green(), color.blue(), color.alpha()]
		})
		.collect();
	std::fs::write(out.join("icon.rgba"), rgba).expect("cannot write the icon");
	println!("cargo::rustc-env=ICON_SIZE={ICON_SIZE}");

	if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
		std::fs::write(out.join("live.kixel.studio.icns"), icns(&tree)).expect("cannot write the bundle icon");
	}
}

/// The icon drawn into a square `size` pixels on an edge.
fn render(tree: &resvg::usvg::Tree, size: u32) -> resvg::tiny_skia::Pixmap {
	let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();
	let scale = size as f32 / tree.size().width().max(tree.size().height());
	resvg::render(
		tree,
		resvg::tiny_skia::Transform::from_scale(scale, scale),
		&mut pixmap.as_mut(),
	);
	pixmap
}

/// The `.icns` Finder and the Dock show: every size macOS asks for, 16 to 512
/// points at 1x and 2x, each a PNG under its type code.
fn icns(tree: &resvg::usvg::Tree) -> Vec<u8> {
	const ENTRIES: [(&[u8; 4], u32); 10] = [
		(b"icp4", 16),
		(b"ic11", 32),
		(b"icp5", 32),
		(b"ic12", 64),
		(b"ic07", 128),
		(b"ic13", 256),
		(b"ic08", 256),
		(b"ic14", 512),
		(b"ic09", 512),
		(b"ic10", 1024),
	];

	// Every length is big-endian and counts its own 8-byte header.
	let mut file = b"icns\0\0\0\0".to_vec();
	for (kind, size) in ENTRIES {
		let png = render(tree, size).encode_png().expect("cannot encode the icon");
		file.extend_from_slice(kind);
		file.extend_from_slice(&(png.len() as u32 + 8).to_be_bytes());
		file.extend_from_slice(&png);
	}
	let len = (file.len() as u32).to_be_bytes();
	file[4..8].copy_from_slice(&len);
	file
}
