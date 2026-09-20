//! Deterministically rasterize Haystack's canonical SVG into every shipped icon size.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const PREVIEW_SIZES: &[u32] = &[16, 20, 24, 32, 64, 128, 256, 512, 1024];
const ICO_SIZES: &[u32] = &[16, 20, 24, 32, 64, 128, 256];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let svg = root.join("haystack-icon.svg");
    let output = root.join("assets/icons/previews");
    fs::create_dir_all(&output)?;
    for &size in PREVIEW_SIZES {
        write_png(&svg, &output.join(format!("haystack_{size}.png")), size)?;
    }
    for (source, name) in [
        ("assets/icons/studies/haystack-h-bars.svg", "h-bars"),
        (
            "assets/icons/studies/haystack-stack-needle.svg",
            "stack-needle",
        ),
    ] {
        let study_output = root.join("assets/icons/studies/previews").join(name);
        fs::create_dir_all(&study_output)?;
        for &size in PREVIEW_SIZES {
            write_png(
                &root.join(source),
                &study_output.join(format!("{size}.png")),
                size,
            )?;
        }
    }
    for &size in &[128, 256, 512] {
        fs::copy(
            output.join(format!("haystack_{size}.png")),
            root.join(format!("assets/icons/haystack_{size}.png")),
        )?;
    }
    fs::copy(
        output.join("haystack_256.png"),
        root.join("src/ui/icons/haystack_256.png"),
    )?;
    fs::copy(
        output.join("haystack_512.png"),
        root.join("src/ui/icons/haystack_512.png"),
    )?;
    write_ico(&output, &root.join("assets/icons/haystack.ico"))?;
    println!("Exported application icon PNGs, previews, and assets/icons/haystack.ico.");
    Ok(())
}

fn write_png(svg: &Path, destination: &Path, size: u32) -> Result<(), Box<dyn std::error::Error>> {
    let data = fs::read(svg)?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default())?;
    let scale = size as f32 / tree.size().width();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| io::Error::other("could not allocate icon pixmap"))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let image = image::RgbaImage::from_raw(size, size, pixmap.data().to_vec())
        .ok_or_else(|| io::Error::other("invalid rendered icon dimensions"))?;
    image.save(destination)?;
    Ok(())
}

/// Windows supports PNG-compressed ICO entries, preserving tested pixels at every size.
fn write_ico(previews: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let payloads: Vec<(u32, Vec<u8>)> = ICO_SIZES
        .iter()
        .map(|size| {
            Ok((
                *size,
                fs::read(previews.join(format!("haystack_{size}.png")))?,
            ))
        })
        .collect::<Result<_, io::Error>>()?;
    let mut file = fs::File::create(destination)?;
    file.write_all(&[0, 0, 1, 0])?;
    file.write_all(&(payloads.len() as u16).to_le_bytes())?;
    let mut offset = 6 + (payloads.len() as u32 * 16);
    for (size, payload) in &payloads {
        let side = if *size == 256 { 0 } else { *size as u8 };
        file.write_all(&[side, side, 0, 0, 1, 0, 32, 0])?;
        file.write_all(&(payload.len() as u32).to_le_bytes())?;
        file.write_all(&offset.to_le_bytes())?;
        offset += payload.len() as u32;
    }
    for (_, payload) in payloads {
        file.write_all(&payload)?;
    }
    Ok(())
}
