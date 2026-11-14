//! The bundled faces, never the machine's: Inter for the interface, Barlow
//! Condensed for state and names, and JetBrains Mono for clocks and rates,
//! each under the SIL Open Font License beside it in `assets/fonts`, cut down
//! by `sh/kit-assets.sh`. egui's own faces stay behind them for the scripts
//! and symbols they lack.

use std::sync::Arc;

use egui::epaint::text::{FontTweak, VariationCoords};
use egui::{FontData, FontDefinitions, FontFamily};

/// The families the kit adds beside egui's proportional and monospace.
pub const DISPLAY: &str = "display";
pub const DISPLAY_BOLD: &str = "display-bold";
pub const MEDIUM: &str = "ui-medium";
pub const STRONG: &str = "ui-strong";

static INTER: &[u8] = include_bytes!("../../assets/fonts/Inter.ttf");
static MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Medium.ttf");
static BARLOW_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/BarlowCondensed-SemiBold.ttf");
static BARLOW_BOLD: &[u8] = include_bytes!("../../assets/fonts/BarlowCondensed-Bold.ttf");

/// Inter at `weight`, one of the weights its variable face holds.
fn inter(weight: f32) -> FontData {
	FontData::from_static(INTER).tweak(FontTweak {
		coords: VariationCoords::new([(b"wght", weight)]),
		..FontTweak::default()
	})
}

/// Every family, each falling back to egui's faces.
pub fn definitions() -> FontDefinitions {
	let mut fonts = FontDefinitions::default();
	let fallback: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
	let faces = [
		("Inter", inter(400.0), FontFamily::Proportional),
		("Inter Medium", inter(500.0), FontFamily::Name(MEDIUM.into())),
		("Inter SemiBold", inter(600.0), FontFamily::Name(STRONG.into())),
		(
			"Barlow Condensed SemiBold",
			FontData::from_static(BARLOW_SEMIBOLD),
			FontFamily::Name(DISPLAY.into()),
		),
		(
			"Barlow Condensed Bold",
			FontData::from_static(BARLOW_BOLD),
			FontFamily::Name(DISPLAY_BOLD.into()),
		),
		("JetBrains Mono", FontData::from_static(MONO), FontFamily::Monospace),
	];
	for (name, data, family) in faces {
		fonts.font_data.insert(name.into(), Arc::new(data));
		let chain = fonts.families.entry(family).or_default();
		// The kit's face first, then egui's, for what it lacks.
		if chain.is_empty() {
			chain.extend(fallback.iter().cloned());
		}
		chain.insert(0, name.into());
	}
	fonts
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Inter's weights are on its weight axis, and every family is there.
	#[test]
	fn the_faces_load() {
		let axes = inter(400.0).variation_axes();
		let weight = axes.iter().find(|axis| axis.tag == "wght");
		let weight = weight.expect("Inter keeps its weight axis");
		assert!(weight.range.min <= 400.0 && weight.range.max >= 600.0, "{weight:?}");
		let families = definitions().families;
		for family in [DISPLAY, DISPLAY_BOLD, MEDIUM, STRONG] {
			assert!(families.contains_key(&FontFamily::Name(family.into())), "{family}");
		}
	}
}
