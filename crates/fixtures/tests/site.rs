//! The fixture site stays one catalogued site (plan 10, milestone 10.1):
//! the index links every top-level folder, and every link in it resolves.

#![allow(clippy::unwrap_used)]

use fixtures::site_dir;

/// The site-local links (`href="/…"`) of `html`, without query or fragment.
fn local_links(html: &str) -> Vec<String> {
    html.split("href=\"/")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(|link| {
            let end = link.find(['?', '#']).unwrap_or(link.len());
            format!("/{}", &link[..end])
        })
        .collect()
}

#[test]
fn the_index_links_every_folder_of_the_site() {
    let index = std::fs::read_to_string(site_dir().join("index.html")).unwrap();
    let links = local_links(&index);
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(site_dir()).unwrap() {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let name = entry.file_name().into_string().unwrap();
        if !links.iter().any(|l| l.starts_with(&format!("/{name}/"))) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "testdata/site/index.html does not link these folders: {missing:?}"
    );
}

#[test]
fn every_link_of_the_index_resolves() {
    let index = std::fs::read_to_string(site_dir().join("index.html")).unwrap();
    for link in local_links(&index) {
        // Dynamic routes of the server, not files.
        if link == "/redirect" {
            continue;
        }
        let mut path = site_dir().join(link.trim_start_matches('/'));
        if path.is_dir() {
            path = path.join("index.html");
        }
        assert!(path.is_file(), "{link} links to a missing file");
    }
}
