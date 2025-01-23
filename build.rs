use reqwest::blocking::get;
use std::fs::File;
use std::io::Write;
use std::path::Path;

fn main() {
    let tlds_path = Path::new("./src/tlds.rs");
    if !tlds_path.exists() {
        // Download TLDs file
        let tlds_url = "https://data.iana.org/TLD/tlds-alpha-by-domain.txt";
        let response = get(tlds_url).expect("Failed to download TLDs file");
        let content = response.text().expect("Failed to read response");

        // Parse and process TLDs
        let tlds = content
            .lines()
            .skip(1)
            .map(|s| s.to_lowercase())
            .collect::<Vec<String>>();

        // Write to output file
        let out_dir = "./src/".to_owned();
        let dest_path = Path::new(&out_dir).join("tlds.rs");
        let mut f = File::create(dest_path).unwrap();

        writeln!(f, "pub const TLDS: &[&str] = &[").unwrap();
        for tld in tlds {
            writeln!(f, "    \"{}\",", tld).unwrap();
        }
        writeln!(f, "];").unwrap();
    } else {
        println!("tlds.rs already exists, skipping download");
    }

    println!("cargo:rerun-if-changed=build.rs");
}
