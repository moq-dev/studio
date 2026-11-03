//! 3D color lookup tables, read from Adobe and Resolve `.cube` files.

/// A 3D lookup table: an output color for each point of a cube of inputs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cube {
	/// Points along each edge of the cube.
	pub size: u32,
	/// The input color at the cube's first corner.
	pub min: [f32; 3],
	/// The input color at the cube's last corner.
	pub max: [f32; 3],
	/// `size³` output colors, red varying fastest, then green, then blue.
	pub table: Vec<[f32; 3]>,
}

impl Cube {
	/// The most points along an edge the format allows.
	const MAX_SIZE: u32 = 256;

	/// Parse a `.cube` file. A 1D table is refused rather than misread as 3D.
	pub fn parse(text: &str) -> Result<Self, String> {
		let invalid = |line: usize, reason: &str| format!("line {}: {reason}", line + 1);
		let mut size = None;
		let mut min = [0.0; 3];
		let mut max = [1.0; 3];
		let mut table = Vec::new();

		for (number, line) in text.lines().enumerate() {
			let line = line.split('#').next().unwrap_or_default().trim();
			let words: Vec<&str> = line.split_whitespace().collect();
			let Some((&first, rest)) = words.split_first() else {
				continue;
			};
			let floats = |words: &[&str]| {
				words
					.iter()
					.map(|word| word.parse::<f32>().ok().filter(|value| value.is_finite()))
					.collect::<Option<Vec<f32>>>()
					.ok_or_else(|| invalid(number, "expected numbers"))
			};
			let keyword = first.starts_with(|c: char| c.is_ascii_alphabetic());
			if keyword && !table.is_empty() {
				return Err(invalid(number, "a keyword after the table"));
			}
			match first {
				"TITLE" => {}
				"LUT_3D_SIZE" => {
					let value = floats(rest)?;
					let edge = match value[..] {
						[edge] if edge.fract() == 0.0 && (2.0..=Self::MAX_SIZE as f32).contains(&edge) => edge as u32,
						_ => return Err(invalid(number, "LUT_3D_SIZE must be from 2 to 256")),
					};
					size = Some(edge);
				}
				"LUT_1D_SIZE" | "LUT_1D_INPUT_RANGE" => {
					return Err(invalid(number, "1D LUTs are not supported; export a 3D one"));
				}
				"DOMAIN_MIN" | "DOMAIN_MAX" => {
					let value: [f32; 3] = floats(rest)?
						.try_into()
						.map_err(|_| invalid(number, "expected three numbers"))?;
					if first == "DOMAIN_MIN" {
						min = value
					} else {
						max = value
					}
				}
				"LUT_3D_INPUT_RANGE" => {
					let [low, high] = floats(rest)?[..] else {
						return Err(invalid(number, "expected two numbers"));
					};
					min = [low; 3];
					max = [high; 3];
				}
				_ if keyword => return Err(invalid(number, &format!("unknown keyword `{first}`"))),
				_ => {
					let Some(size) = size else {
						return Err(invalid(number, "a table entry before LUT_3D_SIZE"));
					};
					if table.len() == (size as usize).pow(3) {
						return Err(invalid(number, "more entries than LUT_3D_SIZE allows"));
					}
					let value: [f32; 3] = floats(&words)?
						.try_into()
						.map_err(|_| invalid(number, "expected an R G B triple"))?;
					table.push(value);
				}
			}
		}

		let size = size.ok_or("no LUT_3D_SIZE")?;
		let expected = (size as usize).pow(3);
		if table.len() != expected {
			return Err(format!(
				"{} entries, but LUT_3D_SIZE {size} needs {expected}",
				table.len()
			));
		}
		if (0..3).any(|channel| min[channel] >= max[channel]) {
			return Err(format!("the domain {min:?} to {max:?} is empty"));
		}
		Ok(Self { size, min, max, table })
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// An identity table of size 2, as written by hand.
	const IDENTITY: &str = "# made by hand
TITLE \"identity\"
LUT_3D_SIZE 2
DOMAIN_MIN 0 0 0
DOMAIN_MAX 1 1 1

0 0 0
1 0 0
0 1 0
1 1 0
0 0 1
1 0 1
0 1 1
1 1 1
";

	#[test]
	fn reads_a_table() {
		let cube = Cube::parse(IDENTITY).unwrap();
		assert_eq!(cube.size, 2);
		assert_eq!(cube.table[1], [1.0, 0.0, 0.0], "red varies fastest");
		assert_eq!(cube.table[2], [0.0, 1.0, 0.0]);

		let resolve = IDENTITY
			.replace("DOMAIN_MIN 0 0 0\n", "")
			.replace("DOMAIN_MAX 1 1 1", "LUT_3D_INPUT_RANGE 0 2");
		assert_eq!(Cube::parse(&resolve).unwrap().max, [2.0; 3]);
	}

	/// A file that is not a whole 3D table fails rather than drawing a guess.
	#[test]
	fn refuses_what_it_cannot_read() {
		let short = IDENTITY.replace("1 1 1\n", "");
		let long = format!("{IDENTITY}1 1 1\n");
		let late = format!("{IDENTITY}DOMAIN_MAX 2 2 2\n");
		let one = IDENTITY.replace("LUT_3D_SIZE 2", "LUT_1D_SIZE 2");
		let huge = IDENTITY.replace("LUT_3D_SIZE 2", "LUT_3D_SIZE 257");
		let pair = IDENTITY.replace("0 1 1", "0 1");
		let empty = IDENTITY.replace("DOMAIN_MAX 1 1 1", "DOMAIN_MAX 0 1 1");
		let unknown = IDENTITY.replace("TITLE", "LUT_FANCY 3\nTITLE");
		let nan = IDENTITY.replace("1 0 1", "1 NaN 1");
		for text in [
			&short, &long, &late, &one, &huge, &pair, &empty, &unknown, &nan, "hello",
		] {
			assert!(Cube::parse(text).is_err(), "{text}");
		}
	}
}
