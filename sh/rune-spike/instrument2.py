# After instrument.py: BENCH_MONITORS docks only Preview and Program in the
# egui window, the same monitors the Rune spike shows. Never committed.
import os, sys

root = sys.argv[1]
p = os.path.join(root, 'rs/moq-studio/src/ui.rs')
s = open(p).read()
anchor = '		let controls = Arc::new(Controls::new(engine.clone(), runtime.clone()));'
assert s.count(anchor) == 1
s = s.replace(anchor, '''		if std::env::var_os("BENCH_MONITORS").is_some() {
			dock = DockState::new(vec![Tab::Preview]);
			dock.main_surface_mut().split_right(NodeIndex::root(), 0.5, vec![Tab::Program]);
		}
''' + anchor)
open(p, 'w').write(s)
print('instrumented monitors')
