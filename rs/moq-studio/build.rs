//! With `cef` on Linux, the binary loads `libcef.so` when it starts: from
//! beside itself, where a package installs the CEF runtime, or else from
//! `CEF_PATH` when the build has one, as the dev shell does.

use std::path::Path;

fn main() {
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
