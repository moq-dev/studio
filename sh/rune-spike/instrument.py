# Temporary frame-time instrumentation, never committed. Run in a nixos
# checkout: python3 instrument.py <root> <egui-wgpu registry dir>
import os, re, shutil, sys

root, registry = sys.argv[1], sys.argv[2]


def edit(path, pairs):
    p = os.path.join(root, path)
    s = open(p).read()
    for old, new in pairs:
        assert s.count(old) == 1, (path, old)
        s = s.replace(old, new)
    open(p, 'w').write(s)


# egui-wgpu, timing what eframe counts as "vsync": acquire, submit, present.
vendor = os.path.join(root, '.tmp/egui-wgpu')
shutil.rmtree(vendor, ignore_errors=True)
shutil.copytree(registry, vendor)
os.chmod(vendor, 0o755)
for dirpath, dirs, files in os.walk(vendor):
    for d in dirs:
        os.chmod(os.path.join(dirpath, d), 0o755)
    for f in files:
        os.chmod(os.path.join(dirpath, f), 0o644)
lib = os.path.join(vendor, 'src/lib.rs')
open(lib, 'a').write('\n/// bench: acquire, submit, present micros, summed.\npub static BENCH_US: [std::sync::atomic::AtomicU64; 3] = [const { std::sync::atomic::AtomicU64::new(0) }; 3];\n')
w = os.path.join(vendor, 'src/winit.rs')
s = open(w).read()
for i, (anchor) in enumerate([
    'let output_frame = surface_state.surface.get_current_texture();\n            vsync_sec += start.elapsed().as_secs_f32();',
    '.submit(core::iter::chain(user_cmd_bufs, [encoded]));\n            vsync_sec += start.elapsed().as_secs_f32();',
    'render_state.queue.present(output_frame);\n            vsync_sec += start.elapsed().as_secs_f32();',
]):
    assert s.count(anchor) == 1, anchor
    s = s.replace(anchor, anchor + f'\n            crate::BENCH_US[{i}].fetch_add(start.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);')
open(w, 'w').write(s)

cargo = os.path.join(root, 'Cargo.toml')
s = open(cargo).read()
if '[patch.crates-io]' in s:
    s = s.replace('[patch.crates-io]', '[patch.crates-io]\negui-wgpu = { path = ".tmp/egui-wgpu" }', 1)
else:
    s += '\n[patch.crates-io]\negui-wgpu = { path = ".tmp/egui-wgpu" }\n'
open(cargo, 'w').write(s)

# A lock that records how long each thread waited for it.
bench = r'''
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Default, Clone, Copy)]
pub struct Acc {
	pub sum: Duration,
	pub max: Duration,
	pub count: u64,
}

impl Acc {
	pub fn add(&mut self, d: Duration) {
		self.sum += d;
		self.max = self.max.max(d);
		self.count += 1;
	}
	pub fn avg_ms(&self) -> f64 {
		if self.count == 0 { 0.0 } else { self.sum.as_secs_f64() * 1e3 / self.count as f64 }
	}
	pub fn max_ms(&self) -> f64 {
		self.max.as_secs_f64() * 1e3
	}
}

pub static WAITS: Mutex<BTreeMap<String, Acc>> = Mutex::new(BTreeMap::new());

pub trait TimedLock<T> {
	fn timed_lock(&self) -> std::sync::LockResult<MutexGuard<'_, T>>;
}

impl<T> TimedLock<T> for Mutex<T> {
	fn timed_lock(&self) -> std::sync::LockResult<MutexGuard<'_, T>> {
		let start = Instant::now();
		let guard = self.lock();
		let waited = start.elapsed();
		let name = std::thread::current().name().unwrap_or("?").to_string();
		WAITS.lock().unwrap().entry(name).or_default().add(waited);
		guard
	}
}

/// The waits a thread has had since the last take.
pub fn take_waits(name: &str) -> Acc {
	WAITS.lock().unwrap().remove(name).unwrap_or_default()
}
'''
open(os.path.join(root, 'rs/moq-studio/src/bench.rs'), 'w').write(bench)

src = os.path.join(root, 'rs/moq-studio/src')
for dirpath, _, files in os.walk(src):
    for f in files:
        if not f.endswith('.rs') or f == 'bench.rs':
            continue
        p = os.path.join(dirpath, f)
        s = open(p).read()
        if '.state.lock()' in s:
            s = s.replace('.state.lock()', '.state.timed_lock()')
            s = '#[allow(unused_imports)]\nuse crate::bench::TimedLock as _;\n' + s if not s.startswith('//!') else re.sub(r'((?://!.*\n)+)', r'\1#[allow(unused_imports)]\nuse crate::bench::TimedLock as _;\n', s, count=1)
            open(p, 'w').write(s)

edit('rs/moq-studio/src/main.rs', [
    ('mod autosave;\n', 'mod autosave;\nmod bench;\n'),
    ('''	fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
		self.fx.begin(ui.ctx());
		self.show(ui, frame);
		self.fx.finish();
	}''', '''	fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
		let start = std::time::Instant::now();
		let cpu = frame.info().cpu_usage.unwrap_or_default();
		self.fx.begin(ui.ctx());
		self.show(ui, frame);
		self.fx.finish();
		ui_bench(start, cpu);
	}'''),
    ('/// What the window shows.\n', '''fn ui_bench(start: std::time::Instant, cpu: f32) {
	use std::sync::atomic::Ordering::Relaxed;
	struct B {
		last: Option<std::time::Instant>,
		interval: bench::Acc,
		ui: bench::Acc,
		cpu: f64,
		wait: bench::Acc,
		gpu: [u64; 3],
		n: u64,
	}
	static B: std::sync::Mutex<Option<B>> = std::sync::Mutex::new(None);
	let mut guard = B.lock().unwrap();
	let b = guard.get_or_insert_with(|| B {
		last: None,
		interval: Default::default(),
		ui: Default::default(),
		cpu: 0.0,
		wait: Default::default(),
		gpu: [0; 3],
		n: 0,
	});
	if let Some(last) = b.last {
		b.interval.add(start - last);
	}
	b.last = Some(start);
	b.ui.add(start.elapsed());
	b.cpu += f64::from(cpu);
	let waits = bench::take_waits("main");
	b.wait.add(waits.sum);
	b.n += 1;
	if b.n == 60 {
		let gpu: Vec<u64> = eframe::egui_wgpu::BENCH_US.iter().map(|us| us.swap(0, Relaxed)).collect();
		let ms = |us: u64| us as f64 / 60.0 / 1e3;
		tracing::info!(
			interval_ms = format!("{:.2} max {:.2}", b.interval.avg_ms(), b.interval.max_ms()),
			ui_ms = format!("{:.2} max {:.2}", b.ui.avg_ms(), b.ui.max_ms()),
			cpu_ms = format!("{:.2}", b.cpu / 60.0 * 1e3),
			lock_wait_ms = format!("{:.2} max {:.2} locks {}", b.wait.avg_ms(), b.wait.max_ms(), waits.count),
			acquire_ms = format!("{:.2}", ms(gpu[0])),
			submit_ms = format!("{:.2}", ms(gpu[1])),
			present_ms = format!("{:.2}", ms(gpu[2])),
			"bench ui"
		);
		*guard = None;
	}
}

/// What the window shows.
'''),
])

edit('rs/moq-studio/src/engine.rs', [
    ('''			let drawn = {
				let mut guard = self.state.timed_lock().unwrap();''', '''			let bench_wait = std::time::Instant::now();
			let drawn = {
				let mut guard = self.state.timed_lock().unwrap();
				let bench_held = std::time::Instant::now();
				bench_program(Some(bench_held - bench_wait), None, None);
				let _bench = BenchHold(bench_held);'''),
    ('''				if let Err(err) = state.compositor.render(&state.collection, &state.sources, tick) {
					tracing::error!(%err, "cannot draw the canvas");
					continue;
				}''', '''				let bench_render = std::time::Instant::now();
				let rendered = state.compositor.render(&state.collection, &state.sources, tick);
				bench_program(None, None, Some(bench_render.elapsed()));
				if let Err(err) = rendered {
					tracing::error!(%err, "cannot draw the canvas");
					continue;
				}'''),
    ('/// The audio the program publishes.\n', '''struct BenchHold(std::time::Instant);

impl Drop for BenchHold {
	fn drop(&mut self) {
		bench_program(None, Some(self.0.elapsed()), None);
	}
}

fn bench_program(wait: Option<Duration>, hold: Option<Duration>, render: Option<Duration>) {
	static B: std::sync::Mutex<[crate::bench::Acc; 3]> = std::sync::Mutex::new([crate::bench::Acc { sum: Duration::ZERO, max: Duration::ZERO, count: 0 }; 3]);
	let mut b = B.lock().unwrap();
	for (i, d) in [wait, hold, render].into_iter().enumerate() {
		if let Some(d) = d {
			b[i].add(d);
		}
	}
	if hold.is_some() && b[1].count == 300 {
		tracing::info!(
			lock_wait_ms = format!("{:.2} max {:.2}", b[0].avg_ms(), b[0].max_ms()),
			hold_ms = format!("{:.2} max {:.2}", b[1].avg_ms(), b[1].max_ms()),
			render_ms = format!("{:.2} max {:.2}", b[2].avg_ms(), b[2].max_ms()),
			"bench program"
		);
		*b = Default::default();
	}
}

/// The audio the program publishes.
'''),
    ('''		let frame = pending.wait().context("copying the program")?;
		load.copy_us
			.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);''', '''		let frame = pending.wait().context("copying the program")?;
		load.copy_us
			.store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
		{
			static B: std::sync::Mutex<crate::bench::Acc> = std::sync::Mutex::new(crate::bench::Acc { sum: Duration::ZERO, max: Duration::ZERO, count: 0 });
			let mut b = B.lock().unwrap();
			b.add(start.elapsed());
			if b.count == 300 {
				tracing::info!(copy_ms = format!("{:.2} max {:.2}", b.avg_ms(), b.max_ms()), "bench copy");
				*b = Default::default();
			}
		}'''),
])
edit('rs/moq-studio/src/fx.rs', [
    ('''	pub fn program(&mut self, spot: Option<&Spot>, on_air: bool, takes: u64) {''', '''	pub fn program(&mut self, spot: Option<&Spot>, on_air: bool, takes: u64) {
		let forced = std::env::var("FX_FORCE").is_ok();
		let on_air = on_air || forced;
		let takes = if forced { takes + (self.time / 2.0) as u64 } else { takes };'''),
    ('''	pub fn health(&mut self, spots: &[&Spot], verdict: Verdict) {''', '''	pub fn health(&mut self, spots: &[&Spot], verdict: Verdict) {
		let verdict = if std::env::var("FX_FORCE").is_ok() { Verdict::Unhealthy } else { verdict };'''),
])
edit('rs/moq-studio/src/ui.rs', [
    ('''		let controls = Arc::new(Controls::new(engine.clone(), runtime.clone()));''', '''		if std::env::var("BENCH_RECORD").is_ok() {
			engine.recorder.start(false).unwrap();
		}
		let controls = Arc::new(Controls::new(engine.clone(), runtime.clone()));'''),
])
print('instrumented')
