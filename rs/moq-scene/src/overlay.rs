//! Overlays: titles, lower thirds, and other graphics written as HTML and CSS,
//! laid out by Blitz and painted by Vello into a texture on the compositor's
//! device, on the program clock.
//!
//! An overlay is untrusted, so it runs nothing and loads only its own assets:
//! Blitz has no script engine or navigation provider, [`check`] refuses any
//! element, attribute, CSS rule, or property outside the subset an overlay
//! draws, naming it, and reads every image and font the overlay names up
//! front, from a `data:` URL or a file under the scene file's directory. Its
//! document fetches from those [`Assets`] alone, never the network.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

use blitz_dom::{BaseDocument, DocumentConfig, FontContext, Node, ns};
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request as Fetch};
use blitz_traits::shell::{ColorScheme, Viewport};
use cssparser::{ParseError, ParseErrorKind, Parser, Token};
use kio::clock::Instant;
use moq_video::render::wgpu;
use style::context::QuirksMode;
use style::error_reporting::{ContextualParseError, ParseErrorReporter};
use style::properties::PropertyDeclarationBlock;
use style::shared_lock::{SharedRwLock, SharedRwLockReadGuard, ToCssWithGuard};
use style::stylesheets::{AllowImportRules, CssRule, CssRuleType, Origin, StylesheetContents, UrlExtraData};
use url::Url;

use crate::Error;
use crate::compositor::FORMAT;

/// Copies an overlay's straight paint into the texture the compositor samples,
/// premultiplied, so scaling it mixes no transparent texel's color.
const PREMULTIPLY: &str = r#"
@group(0) @binding(0) var paint: texture_2d<f32>;

// One triangle covering the whole target.
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
	let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
	return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
	let color = textureLoad(paint, vec2<i32>(position.xy), 0);
	return vec4<f32>(color.rgb * color.a, color.a);
}
"#;

/// The elements an overlay may hold: text, the boxes around it, and images.
/// Anything that loads a document or stylesheet (`link`, `iframe`), runs code
/// (`script`), takes input (`input`, `button`), or draws outside CSS (`svg`,
/// `canvas`) is refused.
const ELEMENTS: &[&str] = &[
	"html",
	"head",
	"title",
	"meta",
	"style",
	"body",
	"div",
	"span",
	"p",
	"h1",
	"h2",
	"h3",
	"h4",
	"h5",
	"h6",
	"header",
	"footer",
	"main",
	"section",
	"article",
	"aside",
	"nav",
	"figure",
	"figcaption",
	"blockquote",
	"pre",
	"code",
	"kbd",
	"samp",
	"var",
	"b",
	"i",
	"u",
	"s",
	"strong",
	"em",
	"small",
	"mark",
	"sub",
	"sup",
	"abbr",
	"cite",
	"q",
	"del",
	"ins",
	"time",
	"data",
	"bdi",
	"bdo",
	"br",
	"wbr",
	"hr",
	"ul",
	"ol",
	"li",
	"dl",
	"dt",
	"dd",
	"table",
	"caption",
	"colgroup",
	"col",
	"thead",
	"tbody",
	"tfoot",
	"tr",
	"th",
	"td",
	"img",
];

/// The attributes an overlay's elements may carry, besides `aria-*` and
/// `data-*`: none names a resource or styles an element outside CSS.
const ATTRIBUTES: &[&str] = &[
	"id", "class", "style", "lang", "dir", "title", "hidden", "charset", "colspan", "rowspan", "span", "start",
	"reversed", "value", "datetime",
];

/// The attributes an `<img>` carries besides [`ATTRIBUTES`]: one image, sized
/// by CSS.
const IMAGE_ATTRIBUTES: &[&str] = &["src", "alt"];

/// The `@font-face` descriptors Blitz honors. It ignores the rest, such as
/// `unicode-range`, so a face would apply where its CSS says it does not.
/// `font-display` changes nothing, since every font loads before the first
/// paint.
const FONT_FACE: &[&str] = &["font-family", "src", "font-weight", "font-style", "font-display"];

/// The CSS properties an overlay may set, as written, shorthands included: the
/// ones Blitz lays out and paints through Vello. Missing are those it ignores
/// or Vello cannot draw yet, such as `filter`, `backdrop-filter`,
/// `mix-blend-mode`, `text-shadow`, `mask`, `text-overflow`, multi-column
/// layout, and logical borders, so an overlay never looks different from what
/// its CSS says.
const PROPERTIES: &[&str] = &[
	// Boxes.
	"display",
	"position",
	"top",
	"right",
	"bottom",
	"left",
	"inset",
	"inset-block",
	"inset-block-start",
	"inset-block-end",
	"inset-inline",
	"inset-inline-start",
	"inset-inline-end",
	"width",
	"height",
	"min-width",
	"min-height",
	"max-width",
	"max-height",
	"block-size",
	"inline-size",
	"min-block-size",
	"min-inline-size",
	"max-block-size",
	"max-inline-size",
	"box-sizing",
	"aspect-ratio",
	"margin",
	"margin-top",
	"margin-right",
	"margin-bottom",
	"margin-left",
	"margin-block",
	"margin-block-start",
	"margin-block-end",
	"margin-inline",
	"margin-inline-start",
	"margin-inline-end",
	"padding",
	"padding-top",
	"padding-right",
	"padding-bottom",
	"padding-left",
	"padding-block",
	"padding-block-start",
	"padding-block-end",
	"padding-inline",
	"padding-inline-start",
	"padding-inline-end",
	"overflow",
	"overflow-x",
	"overflow-y",
	"z-index",
	"float",
	"clear",
	"visibility",
	"direction",
	// Flexbox and grid.
	"flex",
	"flex-flow",
	"flex-direction",
	"flex-wrap",
	"flex-grow",
	"flex-shrink",
	"flex-basis",
	"order",
	"align-items",
	"align-self",
	"align-content",
	"justify-content",
	"justify-items",
	"justify-self",
	"place-items",
	"place-self",
	"place-content",
	"gap",
	"row-gap",
	"column-gap",
	"grid",
	"grid-template",
	"grid-template-columns",
	"grid-template-rows",
	"grid-template-areas",
	"grid-auto-columns",
	"grid-auto-rows",
	"grid-auto-flow",
	"grid-area",
	"grid-row",
	"grid-row-start",
	"grid-row-end",
	"grid-column",
	"grid-column-start",
	"grid-column-end",
	// Tables and lists.
	"border-collapse",
	"border-spacing",
	"table-layout",
	"list-style",
	"list-style-type",
	"list-style-position",
	// Paint.
	"color",
	"opacity",
	"background",
	"background-color",
	"background-image",
	"background-position",
	"background-position-x",
	"background-position-y",
	"background-size",
	"background-repeat",
	"background-clip",
	"background-origin",
	"border",
	"border-width",
	"border-style",
	"border-color",
	"border-top",
	"border-right",
	"border-bottom",
	"border-left",
	"border-top-width",
	"border-right-width",
	"border-bottom-width",
	"border-left-width",
	"border-top-style",
	"border-right-style",
	"border-bottom-style",
	"border-left-style",
	"border-top-color",
	"border-right-color",
	"border-bottom-color",
	"border-left-color",
	"border-radius",
	"border-top-left-radius",
	"border-top-right-radius",
	"border-bottom-right-radius",
	"border-bottom-left-radius",
	"box-shadow",
	"outline",
	"outline-color",
	"outline-style",
	"outline-width",
	"transform",
	"transform-origin",
	"translate",
	"rotate",
	"scale",
	"clip-path",
	"clip",
	// Text.
	"font",
	"font-family",
	"font-size",
	"font-style",
	"font-weight",
	"font-stretch",
	"font-width",
	"font-feature-settings",
	"font-variation-settings",
	"line-height",
	"letter-spacing",
	"word-spacing",
	"text-align",
	"text-indent",
	"text-transform",
	"text-decoration",
	"text-decoration-line",
	"text-decoration-color",
	"text-decoration-style",
	"text-decoration-thickness",
	"text-underline-offset",
	"white-space",
	"white-space-collapse",
	"text-wrap-mode",
	"word-break",
	"overflow-wrap",
	"word-wrap",
	"line-break",
	"vertical-align",
	"content",
	// Motion, on the program clock.
	"animation",
	"animation-name",
	"animation-duration",
	"animation-timing-function",
	"animation-delay",
	"animation-iteration-count",
	"animation-direction",
	"animation-fill-mode",
	"animation-play-state",
	"transition",
	"transition-property",
	"transition-duration",
	"transition-timing-function",
	"transition-delay",
	"transition-behavior",
];

/// An overlay's HTML that passed [`check`], with every asset it loads.
pub(crate) struct Content {
	html: String,
	/// What its relative URLs resolve against: the scene file's directory.
	base: Url,
	/// Each image and font it loads, by the URL its document resolves to.
	assets: HashMap<String, Bytes>,
}

/// What an asset is meant to be, to check that its bytes are one Blitz draws.
#[derive(Clone, Copy)]
enum Kind {
	Image,
	Font,
}

/// The first bytes of a font Blitz loads: TrueType, OpenType, WOFF, or WOFF2.
const FONT_MAGIC: &[&[u8]] = &[b"\0\x01\0\0", b"true", b"OTTO", b"wOFF", b"wOF2"];

/// Refuse `html` unless every element, attribute, CSS rule, and CSS property
/// in it is one an overlay draws, and every image and font it names loads
/// from a `data:` URL or a file under `dir`, the scene file's directory,
/// naming the first that does not.
pub(crate) fn check(html: &str, dir: &Path) -> Result<Content, Error> {
	let dir = std::path::absolute(dir)
		.map_err(|err| Error::Overlay(format!("cannot find the scene file's directory: {err}")))?;
	let base = Url::from_directory_path(&dir)
		.map_err(|()| Error::Overlay(format!("cannot name `{}` as a URL", dir.display())))?;
	// Blitz panics as it parses an element whose `src` or `href` is no URL.
	let document = std::panic::catch_unwind(|| document(html, 1, 1, &base, None))
		.map_err(|_| Error::Overlay("an element has a `src` or `href` that is no URL".into()))?;
	let mut loader = Loader {
		dir,
		content: Content {
			html: html.into(),
			base,
			assets: HashMap::new(),
		},
	};
	loader.check_node(&document, document.root_node())?;
	Ok(loader.content)
}

/// Walks an overlay's HTML and CSS for [`check`], reading each asset it names.
struct Loader {
	dir: PathBuf,
	content: Content,
}

impl Loader {
	fn check_node(&mut self, document: &BaseDocument, node: &Node) -> Result<(), Error> {
		if let Some(element) = node.element_data() {
			let name: &str = &element.name.local;
			if element.name.ns != ns!(html) || !ELEMENTS.contains(&name) {
				return Err(refused(format!("the `<{name}>` element")));
			}
			let image = name == "img";
			for attribute in element.attrs.iter() {
				let attribute: &str = &attribute.name.local;
				let inert = attribute.starts_with("aria-") || attribute.starts_with("data-");
				let allowed = ATTRIBUTES.contains(&attribute) || (image && IMAGE_ATTRIBUTES.contains(&attribute));
				if !inert && !allowed {
					return Err(refused(format!("the `{attribute}` attribute on `<{name}>`")));
				}
			}
			if image {
				let src = node.attr(blitz_dom::local_name!("src")).unwrap_or_default();
				if src.is_empty() {
					return Err(Error::Overlay("an `<img>` has no `src`".into()));
				}
				self.load(src, Kind::Image)?;
			}
			if name == "style" {
				self.check_sheet(&node.text_content())?;
			}
			if let Some(style) = node.attr(blitz_dom::local_name!("style")) {
				self.check_attribute(style)?;
			}
		}
		for &child in &node.children {
			if let Some(child) = document.get_node(child) {
				self.check_node(document, child)?;
			}
		}
		Ok(())
	}

	fn check_sheet(&mut self, css: &str) -> Result<(), Error> {
		let lock = SharedRwLock::new();
		let reporter = Reporter::default();
		let sheet = StylesheetContents::from_str(
			css,
			url(),
			Origin::Author,
			&lock,
			None,
			Some(&reporter),
			QuirksMode::NoQuirks,
			AllowImportRules::No,
			None,
		);
		reporter.result()?;
		let guard = lock.read();
		self.check_rules(&sheet.rules.read_with(&guard).0, &guard, true)
	}

	/// Check `rules`, which are a sheet's own when `top`, where Blitz looks
	/// for `@font-face`.
	fn check_rules(&mut self, rules: &[CssRule], guard: &SharedRwLockReadGuard, top: bool) -> Result<(), Error> {
		for rule in rules {
			match rule {
				CssRule::Style(style) => {
					let style = style.read_with(guard);
					self.check_block(style.block.read_with(guard))?;
					if let Some(nested) = &style.rules {
						self.check_rules(&nested.read_with(guard).0, guard, false)?;
					}
				}
				CssRule::NestedDeclarations(nested) => {
					self.check_block(nested.read_with(guard).block.read_with(guard))?
				}
				CssRule::Media(media) => self.check_rules(&media.rules.read_with(guard).0, guard, false)?,
				CssRule::Keyframes(keyframes) => {
					for keyframe in &keyframes.read_with(guard).keyframes {
						self.check_block(keyframe.read_with(guard).block.read_with(guard))?;
					}
				}
				CssRule::FontFace(_) if top => self.check_font_face(&rule.to_css_string(guard))?,
				CssRule::FontFace(_) => {
					return Err(refused("an `@font-face` rule inside another rule".into()));
				}
				other => {
					let css = other.to_css_string(guard);
					let name = css.split(|c: char| c.is_whitespace() || c == '{' || c == ';').next();
					return Err(refused(format!("the `{}` rule", name.unwrap_or_default())));
				}
			}
		}
		Ok(())
	}

	fn check_attribute(&mut self, css: &str) -> Result<(), Error> {
		let reporter = Reporter::default();
		let block = style::properties::parse_style_attribute(
			css,
			&url(),
			Some(&reporter),
			QuirksMode::NoQuirks,
			CssRuleType::Style,
		);
		reporter.result()?;
		self.check_block(&block)
	}

	/// Check a block's declarations by the names they serialize under, which
	/// are shorthands wherever one covers them, so `border: 1px solid` is
	/// checked as `border` rather than as the `border-image` it resets.
	fn check_block(&mut self, block: &PropertyDeclarationBlock) -> Result<(), Error> {
		let mut css = String::new();
		block.to_css(&mut css).expect("writing to a string");
		self.check_declarations(&css, PROPERTIES, "property", Kind::Image)?;
		Ok(())
	}

	/// Check an `@font-face` rule, as Stylo serializes it, and read its fonts.
	fn check_font_face(&mut self, css: &str) -> Result<(), Error> {
		let body = css
			.split_once('{')
			.and_then(|(_, body)| body.rsplit_once('}'))
			.ok_or_else(|| unreadable(css))?
			.0;
		let named = self.check_declarations(body, FONT_FACE, "`@font-face` descriptor", Kind::Font)?;
		for needed in ["font-family", "src"] {
			if !named.contains(needed) {
				return Err(Error::Overlay(format!("an `@font-face` rule has no `{needed}`")));
			}
		}
		Ok(())
	}

	/// Check that `css`, a run of declarations, names only what `allowed`
	/// lists or custom properties, loading each `url()` in their values as a
	/// `kind`. Returns the names.
	fn check_declarations(
		&mut self,
		css: &str,
		allowed: &[&str],
		what: &str,
		kind: Kind,
	) -> Result<HashSet<String>, Error> {
		let mut named = HashSet::new();
		let mut parser = Parser::new(css);
		while !parser.is_exhausted() {
			let name = parser.expect_ident_cloned().map_err(|_| unreadable(css))?;
			parser.expect_colon().map_err(|_| unreadable(css))?;
			if !name.starts_with("--") && !allowed.contains(&name.as_ref()) {
				return Err(refused(format!("the `{name}` {what}")));
			}
			parser
				.parse_until_after(cssparser::Delimiter::Semicolon, |value| {
					self.load_value(&name, value, kind)
				})
				.map_err(|err| match err.kind {
					ParseErrorKind::Custom(err) => err,
					ParseErrorKind::Basic(_) => unreadable(css),
				})?;
			named.insert(name.to_string());
		}
		Ok(named)
	}

	/// Load each `url()` in `name`'s value as a `kind`, refusing the functions
	/// that load something else.
	fn load_value(&mut self, name: &str, input: &mut Parser, kind: Kind) -> Result<(), ParseError<Error>> {
		while let Ok(token) = input.next() {
			match token.clone() {
				Token::UnquotedUrl(url) => self.load(&url, kind).map_err(ParseError::custom)?,
				Token::Function(function) if function.eq_ignore_ascii_case("url") => {
					let url = input.parse_nested_block(|input| Ok(input.expect_string()?.clone()))?;
					self.load(&url, kind).map_err(ParseError::custom)?;
				}
				Token::Function(function)
					if ["local", "image-set", "-webkit-image-set", "src"]
						.iter()
						.any(|loads| function.eq_ignore_ascii_case(loads)) =>
				{
					let function = function.to_ascii_lowercase();
					return Err(ParseError::custom(Error::Overlay(format!(
						"`{name}` uses `{function}()`, which an overlay does not load"
					))));
				}
				Token::Function(_) | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
					input.parse_nested_block(|input| self.load_value(name, input, kind))?;
				}
				_ => {}
			}
		}
		Ok(())
	}

	/// Read the asset `raw` names, resolved as the overlay's document will,
	/// and check that it is a `kind` Blitz draws.
	fn load(&mut self, raw: &str, kind: Kind) -> Result<(), Error> {
		let failed = |err: String| Error::Overlay(format!("cannot load `{}`: {err}", shorten(raw)));
		let url = self.content.base.join(raw).map_err(|err| failed(err.to_string()))?;
		let bytes = match self.content.assets.get(url.as_str()) {
			Some(bytes) => bytes.clone(),
			None => match url.scheme() {
				"data" => data(&url),
				"file" => self.file(&url),
				_ => Err("an overlay loads only `data:` URLs and files under the scene file's directory".into()),
			}
			.map_err(failed)?,
		};
		match kind {
			Kind::Image => image(&bytes),
			Kind::Font => match bytes.get(..4) {
				Some(magic) if FONT_MAGIC.contains(&magic) => Ok(()),
				_ => Err("it is not a TrueType, OpenType, WOFF, or WOFF2 font".into()),
			},
		}
		.map_err(failed)?;
		self.content.assets.insert(url.into(), bytes);
		Ok(())
	}

	/// Read the file `url` names, if it lies under the scene file's directory
	/// once every symlink and `..` is resolved.
	fn file(&self, url: &Url) -> Result<Bytes, String> {
		let path = url.to_file_path().map_err(|()| "it names no local file".to_string())?;
		let path = std::fs::canonicalize(path).map_err(|err| err.to_string())?;
		let dir =
			std::fs::canonicalize(&self.dir).map_err(|err| format!("cannot find the scene file's directory: {err}"))?;
		if !path.starts_with(&dir) {
			return Err("it is outside the scene file's directory".into());
		}
		std::fs::read(&path).map(Bytes::from).map_err(|err| err.to_string())
	}
}

/// The bytes a `data:` URL holds, decoded as the Fetch standard does.
fn data(url: &Url) -> Result<Bytes, String> {
	use base64::Engine;
	use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

	const FORGIVING: GeneralPurpose = GeneralPurpose::new(
		&base64::alphabet::STANDARD,
		GeneralPurposeConfig::new()
			.with_decode_padding_mode(DecodePaddingMode::Indifferent)
			.with_decode_allow_trailing_bits(true),
	);

	let (meta, body) = url[url::Position::BeforePath..url::Position::AfterQuery]
		.split_once(',')
		.ok_or("it has no `,` before its data")?;
	let body: Vec<u8> = percent_encoding::percent_decode_str(body).collect();
	let meta = meta.trim_ascii();
	let base64 = meta
		.len()
		.checked_sub("base64".len())
		.and_then(|at| meta.split_at_checked(at))
		.is_some_and(|(head, tail)| tail.eq_ignore_ascii_case("base64") && head.trim_end_matches(' ').ends_with(';'));
	if !base64 {
		return Ok(body.into());
	}
	let body: Vec<u8> = body.into_iter().filter(|byte| !byte.is_ascii_whitespace()).collect();
	FORGIVING
		.decode(body)
		.map(Bytes::from)
		.map_err(|err| format!("its base64 is malformed: {err}"))
}

/// Check that `bytes` are a PNG or JPEG no larger than an overlay, so decoding
/// it holds no more than an overlay's own texture.
fn image(bytes: &[u8]) -> Result<(), String> {
	let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
		.with_guessed_format()
		.expect("reading from memory");
	if !matches!(
		reader.format(),
		Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
	) {
		return Err("it is not a PNG or JPEG image".into());
	}
	let (width, height) = reader.into_dimensions().map_err(|err| err.to_string())?;
	let max = crate::browser::MAX_SIDE;
	if width > max || height > max {
		return Err(format!(
			"it is {width}x{height}; each side must be at most {max} pixels"
		));
	}
	Ok(())
}

/// `url` as an error names it, a long `data:` URL cut short.
fn shorten(url: &str) -> String {
	const MAX: usize = 64;
	match url.char_indices().nth(MAX) {
		Some((end, _)) => format!("{}...", &url[..end]),
		None => url.into(),
	}
}

/// Collects the first error Stylo reports, such as an unknown property or an
/// invalid value.
#[derive(Default)]
struct Reporter(RefCell<Option<String>>);

impl ParseErrorReporter for Reporter {
	fn report_error(&self, _url: &UrlExtraData, location: cssparser::SourceLocation, error: ContextualParseError) {
		self.0
			.borrow_mut()
			.get_or_insert_with(|| format!("line {}: {error}", location.line + 1));
	}
}

impl Reporter {
	fn result(self) -> Result<(), Error> {
		match self.0.into_inner() {
			Some(error) => Err(Error::Overlay(format!("CSS {error}"))),
			None => Ok(()),
		}
	}
}

/// The base [`check`]'s CSS parses against; [`Loader::load`] resolves each
/// `url()` itself, as written.
fn url() -> UrlExtraData {
	Url::parse("about:blank").expect("a valid url").into()
}

fn unreadable(css: &str) -> Error {
	Error::Overlay(format!("cannot read back the declarations `{css}`"))
}

fn refused(what: String) -> Error {
	Error::Overlay(format!("{what} is not supported in an overlay"))
}

/// Serves an overlay's document the assets [`check`] read, and nothing else.
struct Assets {
	content: Content,
	/// The first URL the document fetched that [`check`] did not read.
	missing: Mutex<Option<String>>,
}

impl NetProvider for Assets {
	fn fetch(&self, _document: usize, request: Fetch, handler: Box<dyn NetHandler>) {
		let url = request.url.as_str();
		match self.content.assets.get(url) {
			Some(bytes) => handler.bytes(url.into(), bytes.clone()),
			None => {
				self.missing.lock().unwrap().get_or_insert_with(|| shorten(url));
			}
		}
	}
}

/// Parse `html` into a `width` by `height` document whose relative URLs
/// resolve against `base`, fetching through `assets` or, without, nothing.
fn document(html: &str, width: u32, height: u32, base: &Url, assets: Option<Arc<Assets>>) -> BaseDocument {
	let config = DocumentConfig {
		viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
		font_ctx: Some(fonts()),
		base_url: Some(base.to_string()),
		net_provider: assets.map(|assets| assets as Arc<dyn NetProvider>),
		..Default::default()
	};
	blitz_html::HtmlDocument::from_html(html, config).into_inner()
}

impl Assets {
	fn new(content: Content) -> Arc<Self> {
		Arc::new(Self {
			content,
			missing: Mutex::default(),
		})
	}
}

/// Lay out the overlay `assets` serves at `width` by `height`, at the start of
/// its timeline.
fn open(assets: &Arc<Assets>, width: u32, height: u32) -> BaseDocument {
	let Content { html, base, .. } = &assets.content;
	let mut document = document(html, width, height, base, Some(assets.clone()));
	// Stylo starts an animation at the time of the resolve that creates it, so
	// this one starts every animation at the start of the timeline, however
	// late the overlay is first drawn. Assets arrive as they are fetched, from
	// memory, so each is in place by the first paint's resolve: fonts and
	// `<img>` ones by this resolve, and background images, which it fetches,
	// by the next.
	document.resolve(0.0);
	document
}

/// Noto Sans, regular, bold, and italic, as every generic family and any
/// family an overlay loads no font for. The machine's fonts are left out, so
/// an overlay looks the same on any.
fn fonts() -> FontContext {
	use parley::fontique::{Blob, Collection, CollectionOptions, GenericFamily, SourceCache};

	let mut fonts = FontContext {
		source_cache: SourceCache::new_shared(),
		collection: Collection::new(CollectionOptions {
			shared: false,
			system_fonts: false,
		}),
	};
	let mut families = Vec::new();
	for face in [
		notosans::REGULAR_TTF,
		notosans::BOLD_TTF,
		notosans::ITALIC_TTF,
		notosans::BOLD_ITALIC_TTF,
	] {
		let registered = fonts.collection.register_fonts(Blob::new(Arc::new(face)), None);
		families.extend(registered.into_iter().map(|(family, _)| family));
	}
	families.dedup();
	for generic in [
		GenericFamily::SansSerif,
		GenericFamily::Serif,
		GenericFamily::Monospace,
		GenericFamily::Cursive,
		GenericFamily::Fantasy,
		GenericFamily::SystemUi,
		GenericFamily::UiSansSerif,
		GenericFamily::UiSerif,
		GenericFamily::UiMonospace,
		GenericFamily::UiRounded,
	] {
		fonts.collection.set_generic_families(generic, families.iter().copied());
	}
	fonts
}

/// The overlays a compositor draws. Their documents live on a thread of their
/// own, since a Blitz document cannot leave the thread that made it, and a
/// panic there fails the overlays rather than the compositor.
pub(crate) struct Overlays {
	device: wgpu::Device,
	requests: mpsc::Sender<Request>,
	/// What each paint failed on, by overlay.
	painted: mpsc::Receiver<Vec<(u64, String)>>,
	next: u64,
}

enum Request {
	Open {
		id: u64,
		assets: Arc<Assets>,
		/// Where Vello paints, straight.
		paint: wgpu::TextureView,
		/// Where the paint lands premultiplied, for the compositor to sample.
		view: wgpu::TextureView,
		start: Instant,
	},
	Close(u64),
	/// Paint these overlays as of this time on the program clock.
	Paint(Instant, Vec<u64>),
	/// Stop the thread the way a panic in Blitz would.
	#[cfg(test)]
	Panic,
}

impl Overlays {
	/// Start the thread overlays are laid out and painted on, onto `device`.
	pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, Error> {
		let (requests, received) = mpsc::channel();
		let (reply, painted) = mpsc::channel();
		let (device, queue) = (device.clone(), queue.clone());
		std::thread::Builder::new()
			.name("overlays".into())
			.spawn({
				let device = device.clone();
				move || run(&device, &queue, &received, &reply)
			})
			.map_err(|err| Error::Overlay(format!("cannot start the overlay thread: {err}")))?;
		Ok(Self {
			device,
			requests,
			painted,
			next: 0,
		})
	}

	/// Lay out `content` at `width` by `height`, its CSS timeline starting at
	/// `start` on the program clock.
	pub fn open(&mut self, content: Content, width: u32, height: u32, start: Instant) -> Overlay {
		let texture = |label, usage| {
			self.device.create_texture(&wgpu::TextureDescriptor {
				label: Some(label),
				size: wgpu::Extent3d {
					width,
					height,
					depth_or_array_layers: 1,
				},
				mip_level_count: 1,
				sample_count: 1,
				dimension: wgpu::TextureDimension::D2,
				// Gamma-encoded, as the compositor reads.
				format: FORMAT,
				usage: usage | wgpu::TextureUsages::TEXTURE_BINDING,
				view_formats: &[],
			})
		};
		// Vello writes straight alpha, which one pass per paint premultiplies.
		let paint = texture("moq-scene overlay paint", wgpu::TextureUsages::STORAGE_BINDING);
		let texture = texture("moq-scene overlay", wgpu::TextureUsages::RENDER_ATTACHMENT);
		let id = self.next;
		self.next += 1;
		// A thread that stopped fails the next paint, which says so.
		let _ = self.requests.send(Request::Open {
			id,
			assets: Assets::new(content),
			paint: paint.create_view(&Default::default()),
			view: texture.create_view(&Default::default()),
			start,
		});
		Overlay {
			id,
			texture,
			requests: self.requests.clone(),
		}
	}

	/// Paint the overlays named by [`Overlay::id`] as of `now` on the program
	/// clock, where each moved since its last paint, before anything drawn
	/// after this samples them. Returns why each that failed did, by id.
	pub fn paint(&self, now: Instant, overlays: Vec<u64>) -> Result<HashMap<u64, String>, Error> {
		let stopped = || Error::Overlay("the overlay thread stopped".into());
		self.requests
			.send(Request::Paint(now, overlays))
			.map_err(|_| stopped())?;
		let failed = self.painted.recv().map_err(|_| stopped())?;
		Ok(failed.into_iter().collect())
	}
}

#[cfg(test)]
impl Overlays {
	/// Stop the thread as a panic in Blitz would.
	pub fn panic(&self) {
		self.requests.send(Request::Panic).unwrap();
	}
}

/// One open overlay, which closes when dropped.
pub(crate) struct Overlay {
	id: u64,
	texture: wgpu::Texture,
	requests: mpsc::Sender<Request>,
}

impl Overlay {
	/// The overlay's key in what [`Overlays::paint`] returns.
	pub fn id(&self) -> u64 {
		self.id
	}

	/// The texture each paint lands in, premultiplied.
	pub fn texture(&self) -> &wgpu::Texture {
		&self.texture
	}
}

impl Drop for Overlay {
	fn drop(&mut self) {
		let _ = self.requests.send(Request::Close(self.id));
	}
}

/// The overlay thread: serve requests until every sender is gone.
fn run(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	requests: &mpsc::Receiver<Request>,
	painted: &mpsc::Sender<Vec<(u64, String)>>,
) {
	let mut documents = HashMap::new();
	// Created with the first paint, or why it could not be.
	let mut painter: Option<Result<Painter, String>> = None;
	for request in requests {
		match request {
			Request::Open {
				id,
				assets,
				paint,
				view,
				start,
			} => {
				let size = view.texture().size();
				let document = open(&assets, size.width, size.height);
				let document = Document {
					document,
					assets,
					paint,
					view,
					premultiply: None,
					start,
					painted: false,
				};
				documents.insert(id, document);
			}
			Request::Close(id) => {
				documents.remove(&id);
			}
			#[cfg(test)]
			Request::Panic => panic!("a test stopped the overlay thread"),
			Request::Paint(now, ids) => {
				let painter = painter.get_or_insert_with(|| Painter::new(device));
				let failed = ids
					.into_iter()
					.filter_map(|id| {
						let document = documents.get_mut(&id)?;
						let result = match painter {
							Ok(painter) => document.paint(painter, device, queue, now),
							Err(err) => Err(err.clone()),
						};
						result.err().map(|err| (id, err))
					})
					.collect();
				if painted.send(failed).is_err() {
					return;
				}
			}
		}
	}
}

/// Vello's compute pipelines on the compositor's device, and the pass that
/// premultiplies what they paint, compiled once and shared by every overlay.
struct Painter {
	renderer: vello::Renderer,
	scene: vello::Scene,
	premultiply: wgpu::RenderPipeline,
	layout: wgpu::BindGroupLayout,
}

impl Painter {
	fn new(device: &wgpu::Device) -> Result<Self, String> {
		let options = vello::RendererOptions {
			// Area coverage only: compiling the MSAA variants too would slow the
			// first overlay for no visible gain at overlay sizes.
			antialiasing_support: vello::AaSupport::area_only(),
			// Vello compiles on one thread on macOS by default, which took 1 s
			// there before the first overlay drew, against 22 ms on many.
			num_init_threads: None,
			..Default::default()
		};
		let renderer = vello::Renderer::new(device, options).map_err(|err| err.to_string())?;

		let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
			label: Some("moq-scene premultiply"),
			entries: &[wgpu::BindGroupLayoutEntry {
				binding: 0,
				visibility: wgpu::ShaderStages::FRAGMENT,
				ty: wgpu::BindingType::Texture {
					sample_type: wgpu::TextureSampleType::Float { filterable: false },
					view_dimension: wgpu::TextureViewDimension::D2,
					multisampled: false,
				},
				count: None,
			}],
		});
		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label: Some("moq-scene premultiply"),
			bind_group_layouts: &[Some(&layout)],
			immediate_size: 0,
		});
		let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label: Some("moq-scene premultiply"),
			source: wgpu::ShaderSource::Wgsl(PREMULTIPLY.into()),
		});
		let premultiply = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label: Some("moq-scene premultiply"),
			layout: Some(&pipeline_layout),
			vertex: wgpu::VertexState {
				module: &shader,
				entry_point: Some("vs_main"),
				compilation_options: Default::default(),
				buffers: &[],
			},
			primitive: Default::default(),
			depth_stencil: None,
			multisample: Default::default(),
			fragment: Some(wgpu::FragmentState {
				module: &shader,
				entry_point: Some("fs_main"),
				compilation_options: Default::default(),
				targets: &[Some(wgpu::ColorTargetState {
					format: FORMAT,
					blend: None,
					write_mask: wgpu::ColorWrites::ALL,
				})],
			}),
			multiview_mask: None,
			cache: None,
		});
		Ok(Self {
			renderer,
			scene: vello::Scene::new(),
			premultiply,
			layout,
		})
	}

	/// Copy `paint` into `view`, premultiplied, after the work already queued.
	fn premultiply(
		&self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		bind: &wgpu::BindGroup,
		view: &wgpu::TextureView,
	) {
		let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
			label: Some("moq-scene premultiply"),
		});
		{
			let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("moq-scene premultiply"),
				color_attachments: &[Some(wgpu::RenderPassColorAttachment {
					view,
					depth_slice: None,
					resolve_target: None,
					ops: wgpu::Operations {
						load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
						store: wgpu::StoreOp::Store,
					},
				})],
				depth_stencil_attachment: None,
				timestamp_writes: None,
				occlusion_query_set: None,
				multiview_mask: None,
			});
			pass.set_pipeline(&self.premultiply);
			pass.set_bind_group(0, bind, &[]);
			pass.draw(0..3, 0..1);
		}
		queue.submit([encoder.finish()]);
	}
}

/// An overlay's document on the overlay thread, and where it paints.
struct Document {
	document: BaseDocument,
	/// What it fetches from.
	assets: Arc<Assets>,
	/// Where Vello paints, straight.
	paint: wgpu::TextureView,
	/// Where the paint lands premultiplied.
	view: wgpu::TextureView,
	/// `paint` bound for [`Painter::premultiply`], from the first paint on.
	premultiply: Option<wgpu::BindGroup>,
	/// Where its CSS timeline starts on the program clock.
	start: Instant,
	/// Whether the texture holds a paint yet.
	painted: bool,
}

impl Document {
	/// Paint as of `now` on the program clock, unless nothing has moved since
	/// the last paint.
	fn paint(
		&mut self,
		painter: &mut Painter,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		now: Instant,
	) -> Result<(), String> {
		if self.painted && !self.document.is_animating() {
			return Ok(());
		}
		self.document
			.resolve(now.saturating_duration_since(self.start).as_secs_f64());
		if let Some(url) = self.assets.missing.lock().unwrap().as_ref() {
			return Err(format!("the overlay fetched `{url}`, which it did not name"));
		}

		let size = self.paint.texture().size();
		painter.scene.reset();
		let mut scene = anyrender_vello::VelloScenePainter::new(&mut painter.scene);
		blitz_paint::paint_scene(&mut scene, &mut self.document, 1.0, size.width, size.height, 0, 0);
		let params = vello::RenderParams {
			base_color: vello::peniko::Color::TRANSPARENT,
			width: size.width,
			height: size.height,
			antialiasing_method: vello::AaConfig::Area,
		};
		painter
			.renderer
			.render_to_texture(device, queue, &painter.scene, &self.paint, &params)
			.map_err(|err| err.to_string())?;
		let bind = self.premultiply.get_or_insert_with(|| {
			device.create_bind_group(&wgpu::BindGroupDescriptor {
				label: Some("moq-scene premultiply"),
				layout: &painter.layout,
				entries: &[wgpu::BindGroupEntry {
					binding: 0,
					resource: wgpu::BindingResource::TextureView(&self.paint),
				}],
			})
		});
		painter.premultiply(device, queue, bind, &self.view);
		self.painted = true;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn refusal(html: &str) -> String {
		refusal_in(html, Path::new("."))
	}

	/// Why `html` fails with its scene file in `dir`.
	fn refusal_in(html: &str, dir: &Path) -> String {
		match check(html, dir) {
			Ok(_) => panic!("{html} passes"),
			Err(err) => err.to_string(),
		}
	}

	/// A `width` by `height` PNG.
	fn png(width: u32, height: u32) -> Vec<u8> {
		let mut png = Vec::new();
		image::RgbaImage::new(width, height)
			.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
			.unwrap();
		png
	}

	fn base64(bytes: &[u8]) -> String {
		use base64::Engine;
		base64::engine::general_purpose::STANDARD.encode(bytes)
	}

	/// The widths of `document`'s `tag` elements, in order.
	fn widths(document: &BaseDocument, tag: &str) -> Vec<f32> {
		document
			.tree()
			.iter()
			.filter(|(_, node)| node.element_data().is_some_and(|element| &*element.name.local == tag))
			.map(|(_, node)| node.final_layout().size.width)
			.collect()
	}

	#[test]
	fn a_lower_third_passes() {
		check(
			r#"<!doctype html><html><head><meta charset="utf-8"><title>Name</title><style>
				.third {
					position: absolute; left: 40px; bottom: 40px; padding: 12px 24px;
					background: linear-gradient(90deg, #1e3a8a, #2563eb);
					border: 2px solid white; border-radius: 8px;
					font: bold 32px sans-serif; color: white;
					box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
					animation: slide 500ms ease-out both;
					--accent: #facc15;
				}
				.third small { display: block; color: var(--accent); }
				@keyframes slide { from { transform: translateX(-120%); opacity: 0 } to { transform: none; opacity: 1 } }
				@media (min-width: 1000px) { .third { font-size: 48px } }
			</style></head>
			<body><div class="third" data-role="name">Ada Lovelace <small style="opacity: 0.8">Analyst</small></div></body></html>"#,
			Path::new("."),
		)
		.unwrap();
	}

	#[test]
	fn what_runs_or_loads_from_elsewhere_is_refused() {
		for (html, named) in [
			("<script>alert(1)</script>", "`<script>`"),
			(
				r#"<link rel="stylesheet" href="https://example.com/a.css">"#,
				"`<link>`",
			),
			(r#"<iframe src="https://example.com"></iframe>"#, "`<iframe>`"),
			("<svg></svg>", "`<svg>`"),
			(r#"<div onclick="go()">x</div>"#, "`onclick`"),
			(r#"<img src="a.png" srcset="b.png 2x">"#, "`srcset`"),
			(r#"<style>@import "https://example.com/a.css";</style>"#, "@import"),
			(
				r#"<img src="https://example.com/a.png">"#,
				"`https://example.com/a.png`",
			),
			(
				"<style>@font-face { font-family: x; src: url(https://example.com/x.woff2) }</style>",
				"`https://example.com/x.woff2`",
			),
			(
				r#"<div style="background: url(https://example.com/a.png)">x</div>"#,
				"`https://example.com/a.png`",
			),
			(
				r#"<div style="--logo: url('//example.com/a.png')">x</div>"#,
				"`//example.com/a.png`",
			),
			(
				"<style>@font-face { font-family: x; src: local(Arial) }</style>",
				"`local()`",
			),
			(
				r#"<div style="background-image: image-set('a.png' 1x)">x</div>"#,
				"`image-set()`",
			),
			("<img>", "no `src`"),
			(r#"<img src="http://[">"#, "no URL"),
		] {
			let err = refusal(html);
			assert!(err.contains(named), "{html}: {err}");
		}
	}

	/// Images and fonts load from `data:` URLs and files beside the scene
	/// file, and are in place by the first paint's resolve.
	#[test]
	fn assets_load_before_the_first_paint() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::create_dir(dir.path().join("brand")).unwrap();
		std::fs::write(dir.path().join("brand/logo.png"), png(30, 10)).unwrap();
		std::fs::write(dir.path().join("brand/face.ttf"), notosans::BOLD_TTF).unwrap();
		let html = format!(
			r#"<style>
				@font-face {{ font-family: Brand; src: url(brand/face.ttf) format("truetype"); font-display: block }}
				body {{ margin: 0 }}
				span {{ display: inline-block; font: 16px sans-serif }}
				.brand {{ font-family: Brand }}
				div {{ width: 30px; height: 10px; background: url("./brand/logo.png") }}
			</style>
			<img src="data:image/png;base64,{}" alt="">
			<span>Lower third</span><span class="brand">Lower third</span>
			<div></div>"#,
			base64(&png(20, 5)),
		);
		let content = check(&html, dir.path()).unwrap();
		assert_eq!(content.assets.len(), 3);

		let assets = Assets::new(content);
		let mut document = open(&assets, 400, 100);
		document.resolve(0.0);
		assert_eq!(*assets.missing.lock().unwrap(), None);

		assert_eq!(widths(&document, "img"), [20.0]);
		let [regular, brand] = widths(&document, "span")[..] else {
			panic!("two spans");
		};
		assert!(brand > regular * 1.05, "regular {regular}, brand {brand}");
		let (_, div) = document
			.tree()
			.iter()
			.find(|(_, node)| node.element_data().is_some_and(|element| &*element.name.local == "div"))
			.unwrap();
		let background = &div.element_data().unwrap().background_images;
		assert!(
			matches!(&background[..], [Some(image)] if image.status == blitz_dom::node::Status::Ok),
			"the background image is in place"
		);
	}

	#[test]
	fn an_asset_must_be_beside_the_scene_file() {
		let parent = tempfile::tempdir().unwrap();
		let dir = parent.path().join("scenes");
		std::fs::create_dir(&dir).unwrap();
		std::fs::write(parent.path().join("secret.png"), png(1, 1)).unwrap();
		std::fs::write(dir.join("big.png"), png(crate::browser::MAX_SIDE + 1, 1)).unwrap();
		std::fs::write(dir.join("notes.txt"), "hello").unwrap();
		#[cfg(unix)]
		std::os::unix::fs::symlink(parent.path().join("secret.png"), dir.join("link.png")).unwrap();
		let secret = parent.path().join("secret.png");
		let secret = Url::from_file_path(&secret).unwrap();

		let mut cases = vec![
			(r#"<img src="../secret.png">"#.to_string(), "outside"),
			(format!(r#"<img src="{secret}">"#), "outside"),
			(r#"<img src="missing.png">"#.into(), "`missing.png`"),
			(r#"<img src=".">"#.into(), "`.`"),
			(r#"<img src="notes.txt">"#.into(), "not a PNG or JPEG"),
			(r#"<img src="big.png">"#.into(), "4097x1"),
			(
				"<style>@font-face { font-family: x; src: url(notes.txt) }</style>".into(),
				"not a TrueType",
			),
			(r#"<img src="data:image/png;base64,%%%">"#.into(), "base64"),
		];
		#[cfg(unix)]
		cases.push((r#"<img src="link.png">"#.into(), "outside"));
		for (html, named) in cases {
			let err = refusal_in(&html, &dir);
			assert!(err.contains(named), "{html}: {err}");
		}
	}

	#[test]
	fn a_font_face_holds_only_what_blitz_honors() {
		let face = |descriptors: &str| format!("<style>@font-face {{ {descriptors} }}</style>",);
		let font = format!("url(data:font/ttf;base64,{})", base64(notosans::REGULAR_TTF));
		for (html, named) in [
			(
				face(&format!("font-family: x; src: {font}; unicode-range: U+0-7F")),
				"`unicode-range`",
			),
			(face("font-family: x"), "no `src`"),
			(face(&format!("src: {font}")), "no `font-family`"),
			(
				format!("<style>@media (min-width: 1px) {{ @font-face {{ font-family: x; src: {font} }} }}</style>"),
				"inside another rule",
			),
		] {
			let err = refusal(&html);
			assert!(err.contains(named), "{html}: {err}");
		}
		check(
			&face(&format!(
				"font-family: x; src: {font}; font-weight: bold; font-style: italic"
			)),
			Path::new("."),
		)
		.unwrap();
	}

	/// A `data:` URL decodes as the Fetch standard says, base64 or not.
	#[test]
	fn data_urls_decode() {
		for (url, bytes) in [
			("data:,a%20b", &b"a b"[..]),
			("data:text/plain;charset=utf-8,hi", b"hi"),
			("data:;base64,aGk=", b"hi"),
			("data:text/plain ; BASE64,aG k", b"hi"),
			("data:text/plain;base64,aGk=#fragment", b"hi"),
		] {
			assert_eq!(data(&Url::parse(url).unwrap()).unwrap(), bytes, "{url}");
		}
	}

	#[test]
	fn css_outside_the_subset_is_named() {
		for (css, named) in [
			("text-shadow: 0 0 2px black", "`text-shadow`"),
			("filter: blur(2px)", "`filter`"),
			("mix-blend-mode: multiply", "`mix-blend-mode`"),
			("colr: red", "colr"),
			("color: nope", "color"),
		] {
			let err = refusal(&format!(r#"<div style="{css}">x</div>"#));
			assert!(err.contains(named), "{css}: {err}");
			let err = refusal(&format!("<style>div {{ {css} }}</style>"));
			assert!(err.contains(named), "{css}: {err}");
		}
	}

	/// Bold text is set in the bold face rather than the regular one, which
	/// is narrower.
	#[test]
	fn bold_has_a_face_of_its_own() {
		let content = check(
			r#"<span style="display: inline-block">Lower third</span>
			<span style="display: inline-block; font: bold 16px Arial">Lower third</span>"#,
			Path::new("."),
		)
		.unwrap();
		let document = open(&Assets::new(content), 400, 100);
		let [regular, bold] = widths(&document, "span")[..] else {
			panic!("two spans");
		};
		assert!(bold > regular * 1.05, "regular {regular}, bold {bold}");
	}

	#[test]
	fn every_property_listed_is_one_stylo_parses() {
		// Stylo enables some properties, such as grid's, through preferences a
		// document sets.
		check("", Path::new(".")).unwrap();
		for name in PROPERTIES {
			assert!(
				style::properties::PropertyId::parse_enabled_for_all_content(name).is_ok(),
				"{name}"
			);
		}
	}
}
