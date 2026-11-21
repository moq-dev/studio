//! Whether Chromium can sandbox pages on this Linux system, asked before CEF
//! starts, so a browser source fails saying what to change rather than run a
//! page unsandboxed or leave the browser to abort.

use std::ffi::CStr;
use std::io;

/// Fail unless Chromium's namespace sandbox can start: unless the studio runs
/// as a user the kernel lets make a user namespace and another inside it, as
/// Chromium itself checks. Ubuntu's AppArmor, since 23.10, refuses that to any
/// program no profile allows it. Chromium's other sandbox, the setuid-root
/// `chrome-sandbox`, takes root to install, and the copy CEF leaves beside
/// the binary is not setuid, so the studio does not count on it.
pub(super) fn check() -> Result<(), String> {
	// SAFETY: neither can fail.
	let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
	if uid == 0 {
		return Err("Chromium sandboxes no pages for root: run the studio as another user".into());
	}
	// Formatted before forking: the child of a threaded process must not
	// allocate, since another thread may hold the allocator's lock.
	let uid_map = format!("{uid} {uid} 1");
	let gid_map = format!("{gid} {gid} 1");
	// SAFETY: the child makes only async-signal-safe calls and leaves through
	// `_exit`, never returning into Rust.
	let pid = unsafe { libc::fork() };
	if pid == -1 {
		return Err(format!(
			"cannot check for Chromium's sandbox: {}",
			io::Error::last_os_error()
		));
	}
	if pid == 0 {
		// SAFETY: as above.
		unsafe { libc::_exit(nest(&uid_map, &gid_map)) };
	}
	let mut status = 0;
	// SAFETY: `pid` is this process's child, and `status` outlives the call.
	while unsafe { libc::waitpid(pid, &mut status, 0) } == -1 {
		let err = io::Error::last_os_error();
		if err.kind() != io::ErrorKind::Interrupted {
			return Err(format!("cannot check for Chromium's sandbox: {err}"));
		}
	}
	let refused = match libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status)) {
		Some(0) => return Ok(()),
		Some(1) => "refuses to make a user namespace",
		Some(2) => "refuses to map the studio's user into a user namespace",
		Some(3) => "refuses to make a user namespace inside another",
		_ => "crashed checking for user namespaces",
	};
	Err(format!(
		"Chromium sandboxes pages in user namespaces, and this system {refused}: allow them, as the README describes for browser sources on Linux"
	))
}

/// Make a user namespace mapping the studio's user, and another inside it,
/// in a child about to exit. Returns 0, or the step that failed.
fn nest(uid_map: &str, gid_map: &str) -> i32 {
	// SAFETY: plain system calls.
	if unsafe { libc::unshare(libc::CLONE_NEWUSER) } != 0 {
		return 1;
	}
	// An unprivileged user maps its group only once it gives up setgroups.
	if !write(c"/proc/self/setgroups", b"deny")
		|| !write(c"/proc/self/gid_map", gid_map.as_bytes())
		|| !write(c"/proc/self/uid_map", uid_map.as_bytes())
	{
		return 2;
	}
	// SAFETY: as above.
	if unsafe { libc::unshare(libc::CLONE_NEWUSER) } != 0 {
		return 3;
	}
	0
}

/// Write all of `data` to the file at `path`, without allocating.
fn write(path: &CStr, data: &[u8]) -> bool {
	// SAFETY: `path` is a C string, and `data` outlives the calls.
	unsafe {
		let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
		if fd < 0 {
			return false;
		}
		let written = libc::write(fd, data.as_ptr().cast(), data.len());
		libc::close(fd);
		written == data.len() as isize
	}
}
