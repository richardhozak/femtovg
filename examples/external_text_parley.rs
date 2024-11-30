// Copyright 2024 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A simple example that lays out some text using Parley, rasterises the glyph using Swash
//! and and then renders it into a PNG using the `image` crate.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::shadow_unrelated)]

mod helpers;

use femtovg::renderer::OpenGl;
use femtovg::{Canvas, Color, Paint, Path};
use helpers::WindowSurface;
use image::codecs::png::PngEncoder;
use image::{self, Pixel, Rgba, RgbaImage};
use parley::layout::{Alignment, Glyph, GlyphRun, Layout, PositionedLayoutItem};
use parley::style::{FontStack, FontWeight, StyleProperty, TextStyle};
use parley::{FontContext, InlineBox, LayoutContext};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::prelude::{LocationRef, NormalizedCoord, Size};
use skrifa::raw::FontRef as ReadFontsRef;
use skrifa::{GlyphId, MetadataProvider, OutlineGlyph};
use std::fs::File;
use std::sync::Arc;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Scaler, Source, StrikeWith};
use swash::zeno;
use swash::FontRef;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::Window;
use zeno::{Format, Vector};

fn run<W: WindowSurface>(mut canvas: Canvas<W::Renderer>, el: EventLoop<()>, mut surface: W, window: Arc<Window>) {
    // The text we are going to style and lay out
    let text = String::from(
        "Some text here. Let's make it a bit longer so that line wrapping kicks in 😊. And also some اللغة العربية arabic text.\nThis is underline and strikethrough text",
    );

    // The display scale for HiDPI rendering
    let display_scale = 1.0;

    // The width for line wrapping
    let max_advance = Some(200.0 * display_scale);

    // Colours for rendering
    let text_color = Color::rgb(0, 0, 0);
    let bg_color = Rgba([255, 255, 255, 255]);

    // Padding around the output image
    let padding = 20;

    // Create a FontContext, LayoutContext and ScaleContext
    //
    // These are all intended to be constructed rarely (perhaps even once per app (or once per thread))
    // and provide caches and scratch space to avoid allocations
    let mut font_cx = FontContext::new();
    let mut layout_cx = LayoutContext::new();
    let mut scale_cx = ScaleContext::new();

    // Setup some Parley text styles
    let brush_style = StyleProperty::Brush(text_color);
    let font_stack = FontStack::from("system-ui");
    let bold_style = StyleProperty::FontWeight(FontWeight::new(600.0));
    let underline_style = StyleProperty::Underline(true);
    let strikethrough_style = StyleProperty::Strikethrough(true);

    // Creatse a RangedBuilder
    let mut builder = layout_cx.ranged_builder(&mut font_cx, &text, display_scale);

    // Set default text colour styles (set foreground text color)
    builder.push_default(brush_style);

    // Set default font family
    builder.push_default(font_stack);
    builder.push_default(StyleProperty::LineHeight(1.3));
    builder.push_default(StyleProperty::FontSize(16.0));

    // Set the first 4 characters to bold
    builder.push(bold_style, 0..4);

    // Set the underline & stoked style
    builder.push(underline_style, 141..150);
    builder.push(strikethrough_style, 155..168);

    builder.push_inline_box(InlineBox {
        id: 0,
        index: 40,
        width: 50.0,
        height: 50.0,
    });
    builder.push_inline_box(InlineBox {
        id: 1,
        index: 50,
        width: 50.0,
        height: 30.0,
    });

    // Build the builder into a Layout
    // let mut layout: Layout<Color> = builder.build(&text);
    let mut layout: Layout<Color> = builder.build(&text);

    // Perform layout (including bidi resolution and shaping) with start alignment
    layout.break_all_lines(max_advance);
    layout.align(max_advance, Alignment::Start);

    // Create image to render into
    let width = layout.width().ceil() as u32 + (padding * 2);
    let height = layout.height().ceil() as u32 + (padding * 2);
    let mut img = RgbaImage::from_pixel(width, height, bg_color);

    // Write image to PNG file in examples/_output dir
    let output_path = {
        let path = std::path::PathBuf::from(file!());
        let mut path = std::fs::canonicalize(path).unwrap();
        path.pop();
        path.pop();
        path.pop();
        path.push("_output");
        drop(std::fs::create_dir(path.clone()));
        path.push("swash_render.png");
        path
    };
    println!("{}", output_path.display());
    let output_file = File::create(output_path).unwrap();
    let png_encoder = PngEncoder::new(output_file);
    img.write_with_encoder(png_encoder).unwrap();

    let mut pen = PathPen::new();

    el.run(move |event, event_loop_window_target| {
        event_loop_window_target.set_control_flow(winit::event_loop::ControlFlow::Poll);

        match event {
            Event::LoopExiting => event_loop_window_target.exit(),
            Event::WindowEvent { ref event, .. } => match event {
                #[cfg(not(target_arch = "wasm32"))]
                WindowEvent::Resized(physical_size) => {
                    surface.resize(physical_size.width, physical_size.height);
                }
                WindowEvent::CloseRequested => event_loop_window_target.exit(),
                WindowEvent::RedrawRequested { .. } => {
                    let dpi_factor = window.scale_factor() as f32;
                    let size = window.inner_size();
                    canvas.set_size(size.width, size.height, 1.0);
                    canvas.clear_rect(0, 0, size.width, size.height, Color::rgbf(0.9, 0.9, 0.9));

                    // Iterate over laid out lines
                    for line in layout.lines() {
                        // Iterate over GlyphRun's within each line
                        for item in line.items() {
                            match item {
                                PositionedLayoutItem::GlyphRun(glyph_run) => {
                                    // render_glyph_run::<W>(&mut scale_cx, &glyph_run, &mut canvas, padding);
                                    render_glyph_run_outlined::<W>(&glyph_run, &mut pen, &mut canvas, padding);
                                }
                                PositionedLayoutItem::InlineBox(inline_box) => {
                                    let mut path = Path::new();
                                    path.rect(
                                        inline_box.x + padding as f32,
                                        inline_box.y + padding as f32,
                                        inline_box.width,
                                        inline_box.height,
                                    );
                                    canvas.fill_path(&path, &Paint::color(Color::rgba(0, 0, 0, 255)));
                                }
                            };
                        }
                    }

                    surface.present(&mut canvas);
                }
                _ => (),
            },
            Event::AboutToWait => window.request_redraw(),

            _ => (),
        }
    })
    .unwrap();
}

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    helpers::start(1000, 600, "Text demo", true);
    #[cfg(target_arch = "wasm32")]
    helpers::start();
}

fn render_glyph_run_outlined<W: WindowSurface>(
    glyph_run: &GlyphRun<'_, Color>,
    pen: &mut PathPen,
    canvas: &mut Canvas<W::Renderer>,
    padding: u32,
) {
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

    let normalized_coords = run
        .normalized_coords()
        .iter()
        .map(|coord| NormalizedCoord::from_bits(*coord))
        .collect::<Vec<_>>();

    // Get glyph outlines using Skrifa. This can be cached in production code.
    let font_collection_ref = font.data.as_ref();
    let font_ref = ReadFontsRef::from_index(font_collection_ref, font.index).unwrap();
    let outlines = font_ref.outline_glyphs();

    // Iterates over the glyphs in the GlyphRun
    for glyph in glyph_run.glyphs() {
        let glyph_x = run_x + glyph.x + padding as f32;
        let glyph_y = run_y - glyph.y + padding as f32;
        run_x += glyph.advance;

        let glyph_id = GlyphId::from(glyph.id);
        if let Some(glyph_outline) = outlines.get(glyph_id) {
            pen.set_origin(glyph_x, glyph_y);
            pen.set_color(color);
            pen.draw_glyph::<W>(&glyph_outline, font_size, &normalized_coords, canvas);
        }
    }
}

fn render_glyph_run<W: WindowSurface>(
    context: &mut ScaleContext,
    glyph_run: &GlyphRun<'_, Color>,
    canvas: &mut Canvas<W::Renderer>,
    padding: u32,
) {
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
        let glyph_x = run_x + glyph.x + (padding as f32);
        let glyph_y = run_y - glyph.y + (padding as f32);
        run_x += glyph.advance;

        render_glyph::<W>(canvas, &mut scaler, color, glyph, glyph_x, glyph_y);
    }
}

fn render_glyph<W: WindowSurface>(
    canvas: &mut Canvas<W::Renderer>,
    scaler: &mut Scaler<'_>,
    color: Color,
    glyph: Glyph,
    glyph_x: f32,
    glyph_y: f32,
) {
    // Compute the fractional offset
    // You'll likely want to quantize this in a real renderer
    let offset = Vector::new(glyph_x.fract(), glyph_y.fract());

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

    let glyph_width = rendered_glyph.placement.width;
    let glyph_height = rendered_glyph.placement.height;
    let glyph_x = (glyph_x.floor() as i32 + rendered_glyph.placement.left) as u32;
    let glyph_y = (glyph_y.floor() as i32 - rendered_glyph.placement.top) as u32;

    match rendered_glyph.content {
        Content::Mask => {
            let mut i = 0;
            for pixel_y in 0..glyph_height {
                for pixel_x in 0..glyph_width {
                    let x = glyph_x + pixel_x;
                    let y = glyph_y + pixel_y;
                    let alpha = rendered_glyph.data[i];
                    let color = Rgba([
                        (color.r * 255.0) as u8,
                        (color.g * 255.0) as u8,
                        (color.b * 255.0) as u8,
                        alpha,
                    ]);
                    // img.get_pixel_mut(x, y).blend(&color);
                    i += 1;
                }
            }
        }
        Content::SubpixelMask => unimplemented!(),
        Content::Color => {
            let row_size = glyph_width as usize * 4;
            for (pixel_y, row) in rendered_glyph.data.chunks_exact(row_size).enumerate() {
                for (pixel_x, pixel) in row.chunks_exact(4).enumerate() {
                    let x = glyph_x + pixel_x as u32;
                    let y = glyph_y + pixel_y as u32;
                    let color = Rgba(pixel.try_into().expect("Not RGBA"));
                    // img.get_pixel_mut(x, y).blend(&color);
                }
            }
        }
    };
}

struct PathPen {
    path: Path,
    x: f32,
    y: f32,
    color: Color,
}

impl PathPen {
    fn new() -> PathPen {
        PathPen {
            path: Path::new(),
            x: 0.0,
            y: 0.0,
            color: Color::black(),
        }
    }

    fn set_origin(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }

    fn set_color(&mut self, color: Color) {
        self.color = color;
    }

    fn fill_rect<W: WindowSurface>(&mut self, width: f32, height: f32, canvas: &mut Canvas<W::Renderer>) {
        let mut path = Path::new();
        path.rect(self.x, self.y, width, height);
        canvas.fill_path(&path, &Paint::color(self.color));
    }

    fn draw_glyph<W: WindowSurface>(
        &mut self,
        glyph: &OutlineGlyph<'_>,
        size: f32,
        normalized_coords: &[NormalizedCoord],
        canvas: &mut Canvas<W::Renderer>,
    ) {
        let location_ref = LocationRef::new(normalized_coords);
        let settings = DrawSettings::unhinted(Size::new(size), location_ref);
        glyph.draw(settings, self).unwrap();

        let path = core::mem::replace(&mut self.path, Path::new());
        canvas.fill_path(&path, &Paint::color(self.color));
        canvas.stroke_path(&path, &Paint::color(Color::rgbaf(1.0, 1.0, 1.0, 0.5)));
    }
}

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.x + x, self.y - y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.x + x, self.y - y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.path.quad_to(self.x + cx0, self.y - cy0, self.x + x, self.y - y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.path.bezier_to(
            self.x + cx0,
            self.y - cy0,
            self.x + cx1,
            self.y - cy1,
            self.x + x,
            self.y - y,
        );
    }

    fn close(&mut self) {
        self.path.close();
    }
}
