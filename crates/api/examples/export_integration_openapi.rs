use std::env;
use std::error::Error;
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let document =
        serde_json::to_string_pretty(&wareboxes_api::openapi::integration_api_v1())? + "\n";

    match arguments.as_slice() {
        [] => print!("{document}"),
        [path] => fs::write(path, document)?,
        [flag, path] if flag == "--check" => check_document(Path::new(path), &document)?,
        _ => {
            return Err(
                "usage: export_integration_openapi [--check] [path-to-openapi.json]".into(),
            );
        }
    }

    Ok(())
}

fn check_document(path: &Path, expected: &str) -> Result<(), Box<dyn Error>> {
    let actual = fs::read_to_string(path)?;
    if actual != expected {
        return Err(format!(
            "{} is stale; regenerate it with `cargo run -p wareboxes-api --example export_integration_openapi -- {}`",
            path.display(),
            path.display()
        )
        .into());
    }
    Ok(())
}
