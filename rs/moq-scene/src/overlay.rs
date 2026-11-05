//! Overlays: titles, lower thirds, and other graphics written as HTML and CSS,
//! laid out by Blitz and painted by Vello into a texture on the compositor's
//! device, on the program clock.
//!
//! An overlay is untrusted, so it loads and runs nothing: Blitz has no script
//! engine, its document has no network or navigation provider, and [`check`]
//! refuses any element, attribute, CSS rule, or property outside the subset an
//! overlay draws, naming it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, mpsc};

use blitz_dom::{BaseDocument, DocumentConfig, FontContext, Node, ns};
use blitz_traits::shell::{ColorScheme, Viewport};
use kio::clock::Instant;
use moq_video::render::wgpu;
use style::context::QuirksMode;
use style::error_reporting::{ContextualParseError, ParseErrorReporter};
use style::properties::PropertyDeclarationBlock;
use style::shared_lock::{SharedRwLock, SharedRwLockReadGuard, ToCssWithGuard};
use style::stylesheets::{AllowImportRules, CssRule, CssRuleType, Origin, StylesheetContents, UrlExtraData};

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

/// The elements an overlay may hold: text and the boxes around it. Anything
/// that loads a resource (`img`, `link`, `iframe`), runs code (`script`),
/// takes input (`input`, `button`), or draws outside CSS (`svg`, `canvas`) is
/// refused.
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
];

/// The attributes an overlay's elements may carry, besides `aria-*` and
/// `data-*`: none names a resource or styles an element outside CSS.
const ATTRIBUTES: &[&str] = &[
	"id", "class", "style", "lang", "dir", "title", "hidden", "charset", "colspan", "rowspan", "span", "start",
	"reversed", "value", "datetime",
];

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

/// CSS functions that load a resource, which an overlay cannot.
const LOADS: &[&str] = &["url(", "image-set("];

/// Refuse `html` unless every element, attribute, CSS rule, and CSS property
/// in it is one an overlay draws, naming the first that is not.
pub(crate) fn check(html: &str) -> Result<(), Error> {
	let document = document(html, 1, 1);
	check_node(&document, document.root_node())
}

fn check_node(document: &BaseDocument, node: &Node) -> Result<(), Error> {
	if let Some(element) = node.element_data() {
		let name: &str = &element.name.local;
		if element.name.ns != ns!(html) || !ELEMENTS.contains(&name) {
			return Err(refused(format!("the `<{name}>` element")));
		}
		for attribute in element.attrs.iter() {
			let attribute: &str = &attribute.name.local;
			let inert = attribute.starts_with("aria-") || attribute.starts_with("data-");
			if !inert && !ATTRIBUTES.contains(&attribute) {
				return Err(refused(format!("the `{attribute}` attribute on `<{name}>`")));
			}
		}
		if name == "style" {
			check_sheet(&node.text_content())?;
		}
		if let Some(style) = node.attr(blitz_dom::local_name!("style")) {
			check_attribute(style)?;
		}
	}
	for &child in &node.children {
		if let Some(child) = document.get_node(child) {
			check_node(document, child)?;
		}
	}
	Ok(())
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

fn url() -> UrlExtraData {
	url::Url::parse("about:blank").expect("a valid url").into()
}

fn check_sheet(css: &str) -> Result<(), Error> {
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
	check_rules(&sheet.rules.read_with(&guard).0, &guard)
}

fn check_rules(rules: &[CssRule], guard: &SharedRwLockReadGuard) -> Result<(), Error> {
	for rule in rules {
		match rule {
			CssRule::Style(style) => {
				let style = style.read_with(guard);
				check_block(style.block.read_with(guard))?;
				if let Some(nested) = &style.rules {
					check_rules(&nested.read_with(guard).0, guard)?;
				}
			}
			CssRule::NestedDeclarations(nested) => check_block(nested.read_with(guard).block.read_with(guard))?,
			CssRule::Media(media) => check_rules(&media.rules.read_with(guard).0, guard)?,
			CssRule::Keyframes(keyframes) => {
				for keyframe in &keyframes.read_with(guard).keyframes {
					check_block(keyframe.read_with(guard).block.read_with(guard))?;
				}
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

fn check_attribute(css: &str) -> Result<(), Error> {
	let reporter = Reporter::default();
	let block = style::properties::parse_style_attribute(
		css,
		&url(),
		Some(&reporter),
		QuirksMode::NoQuirks,
		CssRuleType::Style,
	);
	reporter.result()?;
	check_block(&block)
}

/// Check a block's declarations by the names they serialize under, which are
/// shorthands wherever one covers them, so `border: 1px solid` is checked as
/// `border` rather than as the `border-image` it resets.
fn check_block(block: &PropertyDeclarationBlock) -> Result<(), Error> {
	let mut css = String::new();
	block.to_css(&mut css).expect("writing to a string");

	let mut parser = cssparser::Parser::new(&css);
	while !parser.is_exhausted() {
		let name = parser.expect_ident_cloned().map_err(|_| unreadable(&css))?;
		parser.expect_colon().map_err(|_| unreadable(&css))?;
		let start = parser.position();
		parser
			.parse_until_after(cssparser::Delimiter::Semicolon, |value| {
				while value.next().is_ok() {}
				Ok::<_, cssparser::ParseError<()>>(())
			})
			.map_err(|_| unreadable(&css))?;
		let value = parser.slice_from(start).to_ascii_lowercase();

		if !name.starts_with("--") && !PROPERTIES.contains(&name.as_ref()) {
			return Err(refused(format!("the `{name}` property")));
		}
		if let Some(load) = LOADS.iter().find(|load| value.contains(*load)) {
			return Err(Error::Overlay(format!(
				"`{name}` uses `{load}`, but an overlay loads nothing"
			)));
		}
	}
	Ok(())
}

fn unreadable(css: &str) -> Error {
	Error::Overlay(format!("cannot read back the declarations `{css}`"))
}

fn refused(what: String) -> Error {
	Error::Overlay(format!("{what} is not supported in an overlay"))
}

/// Parse `html` into a `width` by `height` document with no network access.
fn document(html: &str, width: u32, height: u32) -> BaseDocument {
	let config = DocumentConfig {
		viewport: Some(Viewport::new(width, height, 1.0, ColorScheme::Light)),
		font_ctx: Some(fonts()),
		..Default::default()
	};
	blitz_html::HtmlDocument::from_html(html, config).into_inner()
}

/// Noto Sans, regular, bold, and italic, as every family: an overlay loads no
/// fonts, and leaves the machine's out so that it looks the same on any.
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
		html: String,
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

	/// Lay out `html` at `width` by `height`, its CSS timeline starting at
	/// `start` on the program clock. The HTML has passed [`check`].
	pub fn open(&mut self, html: &str, width: u32, height: u32, start: Instant) -> Overlay {
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
			html: html.to_string(),
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
				html,
				paint,
				view,
				start,
			} => {
				let size = view.texture().size();
				let mut document = document(&html, size.width, size.height);
				// Stylo starts an animation at the time of the resolve that
				// creates it, so this one starts every animation at the start of
				// the timeline, however late the overlay is first drawn.
				document.resolve(0.0);
				let document = Document {
					document,
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
		check(html).unwrap_err().to_string()
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
		)
		.unwrap();
	}

	#[test]
	fn what_loads_or_runs_is_refused() {
		for (html, named) in [
			("<script>alert(1)</script>", "`<script>`"),
			(r#"<img src="https://example.com/a.png">"#, "`<img>`"),
			(
				r#"<link rel="stylesheet" href="https://example.com/a.css">"#,
				"`<link>`",
			),
			(r#"<iframe src="https://example.com"></iframe>"#, "`<iframe>`"),
			("<svg></svg>", "`<svg>`"),
			(r#"<div onclick="go()">x</div>"#, "`onclick`"),
			(r#"<style>@import "https://example.com/a.css";</style>"#, "@import"),
			(
				"<style>@font-face { font-family: x; src: url(https://example.com/x.woff2) }</style>",
				"`@font-face`",
			),
			(
				r#"<div style="background: url(https://example.com/a.png)">x</div>"#,
				"`url(`",
			),
		] {
			let err = refusal(html);
			assert!(err.contains(named), "{html}: {err}");
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
		let mut document = document(
			r#"<span style="display: inline-block">Lower third</span>
			<span style="display: inline-block; font: bold 16px Arial">Lower third</span>"#,
			400,
			100,
		);
		document.resolve(0.0);
		let widths: Vec<f32> = document
			.tree()
			.iter()
			.filter(|(_, node)| node.data.is_element_with_tag_name(&blitz_dom::local_name!("span")))
			.map(|(_, node)| node.final_layout().size.width)
			.collect();
		let [regular, bold] = widths[..] else {
			panic!("two spans: {widths:?}");
		};
		assert!(bold > regular * 1.05, "regular {regular}, bold {bold}");
	}

	#[test]
	fn every_property_listed_is_one_stylo_parses() {
		// Stylo enables some properties, such as grid's, through preferences a
		// document sets.
		document("", 1, 1);
		for name in PROPERTIES {
			assert!(
				style::properties::PropertyId::parse_enabled_for_all_content(name).is_ok(),
				"{name}"
			);
		}
	}
}
