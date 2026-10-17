//! Locating something in an image with the blocking client: a presence check
//! and a box in one request. Box answers always hold a box, even for a target
//! that isn't there, so code checks presence before using it.
//!
//! ```sh
//! TYPESAFE_API_KEY=... cargo run --example locate --features blocking -- photo.jpg "the dog"
//! ```

use serde_json::json;
use typesafe::blocking::Client;
use typesafe::{BoxQuestion, CallOptions, Image, Noul, Questions};

fn main() -> Result<(), typesafe::Error> {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(target)) = (args.next(), args.next()) else {
        eprintln!("usage: locate <image.png|image.jpg> <what to find, such as \"the dog\">");
        std::process::exit(2);
    };
    let client = Client::new();
    if !client.is_configured() {
        eprintln!("set TYPESAFE_API_KEY to run this example");
        std::process::exit(1);
    }

    // The state names the image, and the questions refer to it by that name.
    let photo = Image::from_path(&path)?;
    let questions = Questions::new()
        .ask("present", Noul::new(format!("Is {target} in `photo`?")))
        .ask("box", BoxQuestion::new(format!("{target} in `photo`")));

    // Box questions need jev-1.14.0 or later; jev-preview is the newest.
    let options = CallOptions::new().model("jev-preview").image(&photo);
    let response = client.system_one_with(&json!({ "photo": photo }), &questions, &options)?;

    let present = response.noul("present").unwrap();
    let found = response.bounding_box("box").unwrap();
    println!(
        "{} · {} input tokens",
        response.model, response.usage.input_tokens
    );
    if !present.yes_above(0.5) {
        println!("{target}: probably not in the image ({:.2})", present.noul);
        return Ok(());
    }
    println!(
        "{target}: left {:.2}, top {:.2}, right {:.2}, bottom {:.2} of the image (confidence {})",
        found.left,
        found.top,
        found.right,
        found.bottom,
        found
            .confidence
            .map_or("unknown".into(), |c| format!("{c:.2}")),
    );
    Ok(())
}
