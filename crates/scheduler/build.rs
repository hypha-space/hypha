use std::{env, fs, path::PathBuf};

use clap_markdown::{MarkdownOptions, help_markdown_custom};

#[allow(dead_code)]
mod cli {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cli.rs"));
}

fn main() {
    println!("cargo:rerun-if-changed=src/cli.rs");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let docs_path = out_dir.join(format!("{}-cli.md", env!("CARGO_PKG_NAME")));

    let crate_name = env!("CARGO_PKG_NAME");
    let options = MarkdownOptions::new()
        .title(format!("{crate_name} CLI Reference"))
        .show_footer(false);
    let body = help_markdown_custom::<cli::Cli>(&options);

    let content = format!(
        "<!-- NOTE: Auto-generated. Do not edit manually. -->\n\n{}\n",
        body.trim_end()
    );

    fs::write(&docs_path, content).expect("failed to write CLI docs");
}
