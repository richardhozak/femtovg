// Copyright 2024 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Original example by Parley Authors, modified for femtovg.
//! You can find the original example source code at
//! https://github.com/linebender/parley/blob/7b9a6f938068d37a3e4218a048cda920803c1f89/examples/swash_render/src/main.rs

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::shadow_unrelated)]

use crate::helpers::WindowSurface;
use femtovg::{
    Atlas, Canvas, Color, DrawCommand, GlyphDrawCommands, ImageFlags, ImageId, ImageSource, Paint, Path, Quad,
    Renderer, TextMetrics,
};
use fnv::{FnvBuildHasher, FnvHasher};
use imgref::{Img, ImgRef};
use lru::LruCache;
use parley::{
    layout::{Alignment, Glyph, GlyphRun, Layout, PositionedLayoutItem},
    style::{FontStack, StyleProperty},
    FontContext, LayoutContext,
};
use rgb::RGBA8;
use std::{
    borrow::BorrowMut,
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::Arc,
};
use swash::{
    scale::{image::Content, Render, ScaleContext, Scaler, Source, StrikeWith},
    zeno, FontRef, GlyphId,
};
use winit::{
    event::{Event, WindowEvent},
    event_loop::EventLoop,
    window::Window,
};
use zeno::{Format, Vector};

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct GlyphCacheKey {
    glyph_id: GlyphId,
    font_index: u32,
    size: u32,
    subpixel_offset_x: u8,
    subpixel_offset_y: u8,
}

impl GlyphCacheKey {
    fn new(glyph_id: GlyphId, font_index: u32, font_size: f32, subpixel_offset: Vector) -> Self {
        Self {
            glyph_id,
            font_index,
            size: (font_size * 10.0).trunc() as u32,
            subpixel_offset_x: (subpixel_offset.x * 10.0).trunc() as u8,
            subpixel_offset_y: (subpixel_offset.y * 10.0).trunc() as u8,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct RenderedGlyph {
    texture_index: usize,
    width: u32,
    height: u32,
    offset_x: i32,
    offset_y: i32,
    atlas_x: u32,
    atlas_y: u32,
    color_glyph: bool,
}

#[derive(Default)]
pub struct RenderCache {
    rendered_glyphs: HashMap<GlyphCacheKey, Option<RenderedGlyph>>,
    glyph_textures: Vec<FontTexture>,
}

const TEXTURE_SIZE: usize = 512;

pub struct FontTexture {
    atlas: Atlas,
    image_id: ImageId,
}

#[derive(Copy, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
struct ShapingId {
    size: u32,
    word_hash: u64,
    // font_ids: [Option<FontId>; 8],
}

impl ShapingId {
    fn new(font_size: f32, word: &str, max_width: Option<f32>) -> Self {
        let mut hasher = FnvHasher::default();
        word.hash(&mut hasher);
        if let Some(max_width) = max_width {
            (max_width.trunc() as i32).hash(&mut hasher);
        }

        Self {
            size: (font_size * 10.0).trunc() as u32,
            word_hash: hasher.finish(),
        }
    }
}

type LayoutCache<'a, H> = LruCache<&'a str, Layout<Color>, H>;

pub struct TextCanvas<'a> {
    font_cx: FontContext,
    layout_cx: LayoutContext<Color>,
    layout_cache: LayoutCache<'a, FnvBuildHasher>,
    scale_cx: ScaleContext,
    render_cache: RenderCache,
}

impl<'a> TextCanvas<'a> {
    pub fn new() -> Self {
        Self {
            font_cx: FontContext::new(),
            layout_cx: LayoutContext::new(),
            layout_cache: LruCache::with_hasher(std::num::NonZeroUsize::new(1000).unwrap(), FnvBuildHasher::default()),
            scale_cx: ScaleContext::new(),
            render_cache: RenderCache::default(),
        }
    }

    pub fn fill_text<T: Renderer>(
        &mut self,
        canvas: &mut Canvas<T>,
        x: f32,
        y: f32,
        text: &'a str,
        paint: &Paint,
        max_advance: Option<f32>,
    ) -> (f32, f32) {
        let layout = self.layout_cache.get_or_insert_mut(text, || {
            // The display scale for HiDPI rendering
            let display_scale = 1.0;

            // Colours for rendering
            let text_color = Color::rgb(0, 0, 0);

            // Setup some Parley text styles
            let brush_style = StyleProperty::Brush(text_color);
            let font_stack = FontStack::from("system-ui");

            // Create a RangedBuilder
            let mut builder = self.layout_cx.ranged_builder(&mut self.font_cx, &text, display_scale);

            // Set default text colour styles (set foreground text color)
            builder.push_default(brush_style);

            // Set default font family
            builder.push_default(font_stack);
            builder.push_default(StyleProperty::LineHeight(1.3));
            builder.push_default(StyleProperty::FontSize(16.0));

            // Build the builder into a Layout
            // let mut layout: Layout<Color> = builder.build(&text);
            builder.build(text)
        });

        layout.break_all_lines(max_advance);
        layout.align(max_advance, Alignment::Start);

        // Iterate over laid out lines
        for line in layout.lines() {
            // Iterate over GlyphRun's within each line
            for item in line.items() {
                match item {
                    PositionedLayoutItem::GlyphRun(glyph_run) => {
                        render_glyph_run(
                            &mut self.scale_cx,
                            &mut self.render_cache,
                            &glyph_run,
                            canvas,
                            x,
                            y,
                            paint,
                        );
                    }
                    PositionedLayoutItem::InlineBox(inline_box) => {
                        let mut path = Path::new();
                        path.rect(x + inline_box.x, y + inline_box.y, inline_box.width, inline_box.height);
                        canvas.fill_path(&path, &Paint::color(Color::rgba(0, 0, 0, 255)));
                    }
                };
            }
        }

        (layout.width(), layout.height())
    }
}

fn render_glyph_run<T: Renderer>(
    context: &mut ScaleContext,
    cache: &mut RenderCache,
    glyph_run: &GlyphRun<'_, Color>,
    canvas: &mut Canvas<T>,
    x: f32,
    y: f32,
    paint: &Paint,
) {
    let mut alpha_cmd_map = HashMap::new();
    let mut color_cmd_map = HashMap::new();

    // Resolve properties of the GlyphRun
    let mut run_x = glyph_run.offset();
    let run_y = glyph_run.baseline();
    let style = glyph_run.style();
    let color = style.brush;

    // Get the "Run" from the "GlyphRun"
    let run = glyph_run.run();

    // Resolve properties of the Run
    let font = run.font();
    let font_size = run.font_size();
    let normalized_coords = run.normalized_coords();

    // Convert from parley::Font to swash::FontRef
    let font_ref = FontRef::from_index(font.data.as_ref(), font.index as usize).unwrap();

    // Build a scaler. As the font properties are constant across an entire run of glyphs
    // we can build one scaler for the run and reuse it for each glyph.
    let mut scaler = context
        .builder(font_ref)
        .size(font_size)
        .hint(true)
        .normalized_coords(normalized_coords)
        .build();

    // Iterates over the glyphs in the GlyphRun
    for glyph in glyph_run.glyphs() {
        let glyph_x = x + run_x + glyph.x;
        let glyph_y = y + run_y - glyph.y;
        run_x += glyph.advance;

        // Compute the fractional offset
        // You'll likely want to quantize this in a real renderer
        let offset = Vector::new(glyph_x.fract(), glyph_y.fract());

        let cache_key = GlyphCacheKey::new(glyph.id, font.index, font_size, offset);

        let Some(rendered) = cache.rendered_glyphs.entry(cache_key).or_insert_with(|| {
            let (content, placement, is_color) = render_glyph(&mut scaler, glyph, offset);

            let content_w = placement.width as usize;
            let content_h = placement.height as usize;

            let mut found = None;
            for (texture_index, glyph_atlas) in cache.glyph_textures.iter_mut().enumerate() {
                if let Some((x, y)) = glyph_atlas.atlas.add_rect(content_w, content_h) {
                    found = Some((texture_index, x, y));
                    break;
                }
            }

            let (texture_index, atlas_alloc_x, atlas_alloc_y) = found.unwrap_or_else(|| {
                // if no atlas could fit the texture, make a new atlas tyvm
                // TODO error handling
                let mut atlas = Atlas::new(TEXTURE_SIZE, TEXTURE_SIZE);
                let image_id = canvas
                    .create_image(
                        Img::new(
                            vec![RGBA8::new(0, 0, 0, 0); TEXTURE_SIZE * TEXTURE_SIZE],
                            TEXTURE_SIZE,
                            TEXTURE_SIZE,
                        )
                        .as_ref(),
                        ImageFlags::NEAREST,
                    )
                    .unwrap();
                let texture_index = cache.glyph_textures.len();
                let (x, y) = atlas.add_rect(content_w, content_h).unwrap();
                cache.glyph_textures.push(FontTexture { atlas, image_id });
                (texture_index, x, y)
            });

            canvas
                .update_image::<ImageSource>(
                    cache.glyph_textures[texture_index].image_id,
                    ImgRef::new(&content, content_w, content_h).into(),
                    atlas_alloc_x,
                    atlas_alloc_y,
                )
                .unwrap();

            Some(RenderedGlyph {
                texture_index,
                width: placement.width,
                height: placement.height,
                offset_x: placement.left,
                offset_y: placement.top,
                atlas_x: atlas_alloc_x as u32,
                atlas_y: atlas_alloc_y as u32,
                color_glyph: is_color,
            })
        }) else {
            continue;
        };

        let cmd_map = if rendered.color_glyph {
            &mut color_cmd_map
        } else {
            &mut alpha_cmd_map
        };

        let cmd = cmd_map.entry(rendered.texture_index).or_insert_with(|| DrawCommand {
            image_id: cache.glyph_textures[rendered.texture_index].image_id,
            quads: Vec::new(),
        });

        let mut q = Quad::default();
        let it = 1.0 / TEXTURE_SIZE as f32;

        q.x0 = glyph_x + rendered.offset_x as f32 - offset.x;
        q.y0 = glyph_y - rendered.offset_y as f32 - offset.y;
        q.x1 = q.x0 + rendered.width as f32;
        q.y1 = q.y0 + rendered.height as f32;

        q.s0 = rendered.atlas_x as f32 * it;
        q.t0 = rendered.atlas_y as f32 * it;
        q.s1 = (rendered.atlas_x + rendered.width) as f32 * it;
        q.t1 = (rendered.atlas_y + rendered.height) as f32 * it;

        cmd.quads.push(q);
    }

    canvas.draw_glyph_commands(
        GlyphDrawCommands {
            alpha_glyphs: alpha_cmd_map.into_values().collect(),
            color_glyphs: color_cmd_map.into_values().collect(),
        },
        paint,
        1.0,
    );
}

fn render_glyph(scaler: &mut Scaler<'_>, glyph: Glyph, offset: Vector) -> (Vec<RGBA8>, zeno::Placement, bool) {
    // Render the glyph using swash
    let rendered_glyph = Render::new(
        // Select our source order
        &[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ],
    )
    // Select the simple alpha (non-subpixel) format
    .format(Format::Alpha)
    // Apply the fractional offset
    .offset(offset)
    // Render the image
    .render(scaler, glyph.id)
    .unwrap();

    let glyph_width = rendered_glyph.placement.width as usize;
    let glyph_height = rendered_glyph.placement.height as usize;

    let mut src_buf = Vec::with_capacity(glyph_width * glyph_height);
    match rendered_glyph.content {
        Content::Mask => {
            for chunk in rendered_glyph.data.chunks_exact(1) {
                src_buf.push(RGBA8::new(chunk[0], 0, 0, 0));
            }
        }
        Content::Color => {
            for chunk in rendered_glyph.data.chunks_exact(4) {
                src_buf.push(RGBA8::new(chunk[0], chunk[1], chunk[2], chunk[3]));
            }
        }
        Content::SubpixelMask => unreachable!(),
    }

    (
        src_buf,
        rendered_glyph.placement,
        matches!(rendered_glyph.content, Content::Color),
    )
}

const LOREM_TEXT: &str = r"
Traditionally, text is composed to create a readable, coherent, and visually satisfying typeface
that works invisibly, without the awareness of the reader. Even distribution of typeset material,
with a minimum of distractions and anomalies, is aimed at producing clarity and transparency.
Choice of typeface(s) is the primary aspect of text typography—prose fiction, non-fiction,
editorial, educational, religious, scientific, spiritual, and commercial writing all have differing
characteristics and requirements of appropriate typefaces and their fonts or styles.

مرئية وساهلة قراءة وجاذبة. ترتيب الحوف يشمل كل من اختيار عائلة الخط وحجم وطول الخط والمسافة بين السطور

مرئية وساهلة قراءة وجاذبة. ترتيب الحوف يشمل كل من اختيار (asdasdasdasdasdasd) عائلة الخط وحجم وطول الخط والمسافة بين السطور

Lorem ipsum dolor sit amet, consectetur adipiscing elit. Curabitur in nisi at ligula lobortis pretium. Sed vel eros tincidunt, fermentum metus sit amet, accumsan massa. Vestibulum sed elit et purus suscipit
Sed at gravida lectus. Duis eu nisl non sem lobortis rutrum. Sed non mauris urna. Pellentesque suscipit nec odio eu varius. Quisque lobortis elit in finibus vulputate. Mauris quis gravida libero.
Etiam non malesuada felis, nec fringilla quam.

😂🤩🥰😊😄
";
