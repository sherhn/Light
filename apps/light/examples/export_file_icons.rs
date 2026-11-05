//! Rebuild the Explorer file icons from the same SVG marks used inside Light.
use std::{fs::{self,File},path::PathBuf};
use image::codecs::ico::{IcoEncoder,IcoFrame};
#[path = "../src/module_marks.rs"]
#[allow(dead_code)]
mod module_marks;

fn main() {
    let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let types:serde_json::Value=serde_json::from_str(&fs::read_to_string(root.join("assets/file-types.json")).unwrap()).unwrap();
    let directory=root.join("assets/file-icons");fs::create_dir_all(&directory).unwrap();
    for kind in types.as_array().unwrap() {
        let id=kind["id"].as_str().unwrap();
        let color=match id { "lisp"|"css"|"yaml"=>"#71568B", "python"|"javascript"|"json"=>"#8A6924", "html"|"assembler"|"toml"=>"#9B542F", "shell-make"|"dotenv"|"ini"=>"#3E704C", _=>"#356F94" };
        let source=module_marks::source(id).unwrap_or(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path fill="none" stroke="black" stroke-width="2" d="m8 5-6 7 6 7m8-14 6 7-6 7M14 3l-4 18"/></svg>"#);
        let mark=source.replacen("<svg",&format!("<svg x=\"56\" y=\"80\" width=\"144\" height=\"144\" fill=\"{color}\""),1).replace("stroke=\"black\"",&format!("stroke=\"{color}\""));
        let svg=format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256"><path fill="#F2F5F1" stroke="#66756B" stroke-width="7" stroke-linejoin="round" d="M38 10h114l66 66v168H38z"/><path fill="#D9E2DA" stroke="#66756B" stroke-width="6" stroke-linejoin="round" d="M152 10v66h66"/>{mark}</svg>"##);
        fs::write(directory.join(format!("{id}.svg")),&svg).unwrap();
        let tree=resvg::usvg::Tree::from_str(&svg,&Default::default()).unwrap();
        let mut frames=Vec::new();
        for size in [16,24,32,48,64,128,256] {
            let mut pixels=resvg::tiny_skia::Pixmap::new(size,size).unwrap();
            resvg::render(&tree,resvg::tiny_skia::Transform::from_scale(size as f32/256.0,size as f32/256.0),&mut pixels.as_mut());
            // tiny-skia stores premultiplied pixels; PNG/ICO use straight alpha.
            let rgba:Vec<u8>=pixels.pixels().iter().flat_map(|p| { let c=p.demultiply();[c.red(),c.green(),c.blue(),c.alpha()] }).collect();
            frames.push(IcoFrame::as_png(&rgba,size,size,image::ExtendedColorType::Rgba8).unwrap());
        }
        IcoEncoder::new(File::create(directory.join(format!("{id}.ico"))).unwrap()).encode_images(&frames).unwrap();
        println!("Exported {id}: 7 icon sizes");
    }
}
