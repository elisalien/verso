//! Online shader libraries: search and download from the ISF shader site
//! (editor.isf.video, no account needed) and ShaderToy (needs a free API key
//! from the user's ShaderToy account; only shaders published as "Public+API"
//! are reachable).
//!
//! Downloads become files the effect library loads: `.fs` for ISF, `.glsl`
//! for ShaderToy code with name, author and link in a comment header for
//! attribution. All calls block; run them off the UI thread.

use std::path::{Path, PathBuf};

use serde::Deserialize;

const USER_AGENT: &str = concat!(
    "Verso/",
    env!("CARGO_PKG_VERSION"),
    " (open source VJ software)"
);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(String),
    #[error("{0}")]
    Api(String),
    #[error("could not save: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

fn http(e: ureq::Error) -> Error {
    match e {
        ureq::Error::StatusCode(403) => Error::Api("access denied (403): check the API key".into()),
        other => Error::Http(other.to_string()),
    }
}

/// Whether a shader may be used in paid work, from license statements in
/// its code. A heuristic: the shader page is the reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Usage {
    /// A license allowing commercial use was found (named).
    Commercial(String),
    /// A non-commercial license was found or applies by default (named).
    NonCommercial(String),
    /// No license statement.
    Unknown,
}

impl Usage {
    pub fn allows_commercial(&self) -> bool {
        matches!(self, Usage::Commercial(_))
    }

    pub fn label(&self) -> &str {
        match self {
            Usage::Commercial(name) | Usage::NonCommercial(name) => name,
            Usage::Unknown => "license unknown",
        }
    }
}

/// Finds a license statement in shader source. Non-commercial terms win over
/// permissive ones, so a dual mention stays on the safe side.
pub fn detect_license(text: &str) -> Option<Usage> {
    let lower = text.to_lowercase();
    const NON_COMMERCIAL: [(&str, &str); 6] = [
        ("by-nc-sa", "CC BY-NC-SA"),
        ("by-nc", "CC BY-NC"),
        ("by nc", "CC BY-NC"),
        ("noncommercial", "Non-commercial"),
        ("non-commercial", "Non-commercial"),
        ("non commercial", "Non-commercial"),
    ];
    const COMMERCIAL: [(&str, &str); 14] = [
        ("cc0", "CC0"),
        ("public domain", "Public domain"),
        ("unlicense", "Unlicense"),
        ("mit license", "MIT"),
        ("mit licence", "MIT"),
        ("license: mit", "MIT"),
        ("licensed under mit", "MIT"),
        ("apache license", "Apache"),
        ("bsd license", "BSD"),
        ("bsd-", "BSD"),
        ("zlib license", "zlib"),
        ("wtfpl", "WTFPL"),
        ("cc by 4.0", "CC BY 4.0"),
        ("cc-by 4.0", "CC BY 4.0"),
    ];
    if let Some((_, name)) = NON_COMMERCIAL.iter().find(|(p, _)| lower.contains(p)) {
        return Some(Usage::NonCommercial((*name).into()));
    }
    if let Some((_, name)) = COMMERCIAL.iter().find(|(p, _)| lower.contains(p)) {
        return Some(Usage::Commercial((*name).into()));
    }
    for (pattern, name) in [
        ("creative commons attribution 4.0", "CC BY 4.0"),
        ("creative commons attribution 3.0", "CC BY 3.0"),
        ("cc by-sa", "CC BY-SA"),
        ("cc by 3.0", "CC BY 3.0"),
    ] {
        if lower.contains(pattern) {
            return Some(Usage::Commercial(name.into()));
        }
    }
    None
}

/// Where a shader comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Site {
    Isf,
    ShaderToy,
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub site: Site,
    pub id: String,
    pub title: String,
    pub author: String,
    /// Page of the shader on its site.
    pub url: String,
    /// Shader file contents, ready to save, when the search returned it.
    pub contents: Option<String>,
    pub usage: Usage,
}

impl Listing {
    /// File name to save under: a readable slug plus the site id.
    pub fn file_name(&self) -> String {
        let slug: String = self
            .title
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let slug = if slug.is_empty() {
            "shader".into()
        } else {
            slug
        };
        let extension = match self.site {
            Site::Isf => "fs",
            Site::ShaderToy => "glsl",
        };
        let id: String = self
            .id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        format!("{}-{id}.{extension}", &slug[..slug.len().min(40)])
    }
}

/// Writes `contents` as `dir/name`, creating `dir`, and returns the path.
pub fn save(dir: &Path, name: &str, contents: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    std::fs::write(&path, contents)?;
    Ok(path)
}

pub mod isf {
    use super::*;

    const API: &str = "https://editor.isf.video/api/search";

    #[derive(Deserialize)]
    struct Page {
        shaders: Vec<Shader>,
        #[serde(rename = "numberOfResults")]
        total: usize,
    }

    #[derive(Deserialize)]
    struct Shader {
        id: String,
        title: String,
        #[serde(rename = "rawFragmentSource")]
        source: Option<String>,
        username: Option<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Sort {
        Popular,
        Recent,
    }

    /// Searches shaders whose title or description matches `query` (empty for
    /// all). `from` is the index of the first result. Returns the listings and
    /// the total number of matches.
    pub fn search(query: &str, sort: Sort, from: usize) -> Result<(Vec<Listing>, usize)> {
        let sort = match sort {
            Sort::Popular => "Popularity ↓",
            Sort::Recent => "Date Created ↓",
        };
        let page: Page = ureq::get(API)
            .header("User-Agent", USER_AGENT)
            .query("q", query)
            .query("category", "")
            .query("sort", sort)
            .query("from", from.to_string())
            .call()
            .map_err(http)?
            .body_mut()
            .read_json()
            .map_err(http)?;
        Ok((page.shaders.into_iter().map(listing).collect(), page.total))
    }

    fn listing(shader: Shader) -> Listing {
        Listing {
            site: Site::Isf,
            url: format!("https://editor.isf.video/shaders/{}", shader.id),
            author: shader.username.unwrap_or_default(),
            title: shader.title.trim().to_owned(),
            usage: shader
                .source
                .as_deref()
                .and_then(detect_license)
                .unwrap_or(Usage::Unknown),
            contents: shader.source,
            id: shader.id,
        }
    }

    #[cfg(test)]
    pub(crate) fn parse_page(json: &str) -> (Vec<Listing>, usize) {
        let page: Page = serde_json::from_str(json).unwrap();
        (page.shaders.into_iter().map(listing).collect(), page.total)
    }
}

pub mod shadertoy {
    use super::*;

    const API: &str = "https://www.shadertoy.com/api/v1";

    #[derive(Deserialize)]
    struct Query {
        #[serde(rename = "Results", default)]
        results: Vec<String>,
        #[serde(rename = "Shaders", default)]
        total: usize,
        #[serde(rename = "Error")]
        error: Option<String>,
    }

    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "Shader")]
        shader: Option<Shader>,
        #[serde(rename = "Error")]
        error: Option<String>,
    }

    #[derive(Deserialize)]
    struct Shader {
        info: Info,
        renderpass: Vec<Pass>,
    }

    #[derive(Deserialize)]
    struct Info {
        id: String,
        name: String,
        username: String,
    }

    #[derive(Deserialize)]
    struct Pass {
        #[serde(rename = "type")]
        kind: String,
        code: String,
        #[serde(default)]
        inputs: Vec<Input>,
    }

    #[derive(Deserialize)]
    struct Input {
        channel: u32,
        ctype: String,
    }

    /// Ids of shaders matching `query`, from index `from`, at most `count`,
    /// and the total number of matches.
    pub fn search(
        key: &str,
        query: &str,
        from: usize,
        count: usize,
    ) -> Result<(Vec<String>, usize)> {
        let url = format!("{API}/shaders/query/{}", encode(query));
        let response: Query = ureq::get(&url)
            .header("User-Agent", USER_AGENT)
            .query("key", key)
            .query("from", from.to_string())
            .query("num", count.to_string())
            .call()
            .map_err(http)?
            .body_mut()
            .read_json()
            .map_err(http)?;
        if let Some(error) = response.error {
            return Err(Error::Api(error));
        }
        Ok((response.results, response.total))
    }

    /// Downloads one shader and converts it to a Verso `.glsl` file.
    pub fn fetch(key: &str, id: &str) -> Result<Listing> {
        let url = format!("{API}/shaders/{}", encode(id));
        let body = ureq::get(&url)
            .header("User-Agent", USER_AGENT)
            .query("key", key)
            .call()
            .map_err(http)?
            .body_mut()
            .read_to_string()
            .map_err(http)?;
        parse(&body)
    }

    /// Converts an API response for one shader.
    pub fn parse(json: &str) -> Result<Listing> {
        let envelope: Envelope = serde_json::from_str(json)
            .map_err(|e| Error::Api(format!("unexpected response: {e}")))?;
        if let Some(error) = envelope.error {
            return Err(Error::Api(error));
        }
        let shader = envelope
            .shader
            .ok_or_else(|| Error::Api("empty response".into()))?;
        let url = format!("https://www.shadertoy.com/view/{}", shader.info.id);
        let code: String = shader.renderpass.iter().map(|p| p.code.as_str()).collect();
        let usage = detect_license(&code)
            .unwrap_or_else(|| Usage::NonCommercial("CC BY-NC-SA 3.0 (ShaderToy default)".into()));
        let contents = to_glsl(&shader, &url);
        Ok(Listing {
            usage,
            site: Site::ShaderToy,
            id: shader.info.id,
            title: shader.info.name,
            author: shader.info.username,
            url,
            contents: Some(contents),
        })
    }

    /// Joins the Common and Image passes under an attribution header. Returns
    /// a file the effect library rejects with a clear message when the shader
    /// needs features Verso lacks (buffers, sound, keyboard, more channels).
    fn to_glsl(shader: &Shader, url: &str) -> String {
        let mut problems = Vec::new();
        let mut image = None;
        let mut common = String::new();
        for pass in &shader.renderpass {
            match pass.kind.as_str() {
                "image" => image = Some(pass),
                "common" => common = pass.code.clone(),
                other => problems.push(format!("uses a {other} pass")),
            }
        }
        if let Some(image) = image {
            for input in &image.inputs {
                let supported = input.channel == 0
                    && matches!(input.ctype.as_str(), "texture" | "video" | "webcam");
                if !supported {
                    problems.push(format!(
                        "reads {} on iChannel{}",
                        input.ctype, input.channel
                    ));
                }
            }
        }
        let mut out = format!(
            "// name: {}\n// author: {}\n// url: {url}\n// license: see the shader page (ShaderToy default: CC BY-NC-SA 3.0)\n",
            shader.info.name.replace('\n', " "),
            shader.info.username,
        );
        if !problems.is_empty() || image.is_none() {
            // Not loadable: keep the reason where the library will report it.
            let reason = if image.is_none() {
                "has no image pass".into()
            } else {
                problems.join(", ")
            };
            out.push_str(&format!(
                "// Not supported by Verso: {reason}.\n#error {reason}\n"
            ));
            return out;
        }
        out.push_str(&common);
        out.push('\n');
        out.push_str(&image.expect("checked").code);
        out.push('\n');
        out
    }

    fn encode(text: &str) -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn encode_for_test(text: &str) -> String {
        encode(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isf_page_parses() {
        let (listings, total) = isf::parse_page(
            r#"{"shaders":[{"id":"abc123","title":" Kaleido ","rawFragmentSource":"/*{}*/ void main(){}","user":"5fc5","username":"ann"}],"numberOfResults":77}"#,
        );
        assert_eq!(total, 77);
        assert_eq!(listings[0].title, "Kaleido");
        assert_eq!(listings[0].author, "ann");
        assert_eq!(listings[0].url, "https://editor.isf.video/shaders/abc123");
        assert_eq!(listings[0].file_name(), "kaleido-abc123.fs");
    }

    #[test]
    fn shadertoy_single_pass_converts() {
        let listing = shadertoy::parse(
            r#"{"Shader":{"ver":"0.1","info":{"id":"XsX3RB","name":"Waves","username":"bob","description":""},
            "renderpass":[
              {"inputs":[],"outputs":[],"code":"float f(float x){return x;}","name":"Common","type":"common"},
              {"inputs":[{"id":1,"src":"/media/a.png","ctype":"texture","channel":0}],"outputs":[],"code":"void mainImage(out vec4 o, in vec2 p){o=texture(iChannel0,p/iResolution.xy);}","name":"Image","type":"image"}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(listing.title, "Waves");
        assert_eq!(listing.file_name(), "waves-XsX3RB.glsl");
        let contents = listing.contents.unwrap();
        assert!(contents.starts_with(
            "// name: Waves\n// author: bob\n// url: https://www.shadertoy.com/view/XsX3RB\n"
        ));
        assert!(contents.contains("float f(float x)"));
        assert!(contents.contains("mainImage"));
        assert!(!contents.contains("#error"));
    }

    #[test]
    fn shadertoy_unsupported_features_are_marked() {
        let listing = shadertoy::parse(
            r#"{"Shader":{"info":{"id":"x","name":"Multi","username":"c"},
            "renderpass":[
              {"inputs":[{"channel":1,"ctype":"buffer"}],"code":"void mainImage(out vec4 o, in vec2 p){}","type":"image"},
              {"inputs":[],"code":"void mainImage(out vec4 o, in vec2 p){}","type":"buffer"}
            ]}}"#,
        )
        .unwrap();
        let contents = listing.contents.unwrap();
        assert!(contents.contains("#error"));
        assert!(contents.contains("buffer pass"));
        assert!(contents.contains("iChannel1"));
    }

    #[test]
    fn licenses_are_detected() {
        let usage = |text| detect_license(text);
        assert_eq!(
            usage("// License: MIT"),
            Some(Usage::Commercial("MIT".into()))
        );
        assert_eq!(
            usage("// Released to the Public Domain"),
            Some(Usage::Commercial("Public domain".into()))
        );
        assert_eq!(usage("// CC0 1.0"), Some(Usage::Commercial("CC0".into())));
        assert_eq!(
            usage("// Creative Commons Attribution-NonCommercial-ShareAlike 3.0"),
            Some(Usage::NonCommercial("Non-commercial".into()))
        );
        // A non-commercial mention wins over a permissive one.
        assert!(
            !usage("// MIT license for the noise, rest CC BY-NC-SA")
                .unwrap()
                .allows_commercial()
        );
        assert_eq!(usage("float f(float x) { return x; }"), None);
    }

    #[test]
    fn shadertoy_default_license_is_non_commercial() {
        let listing = shadertoy::parse(
            r#"{"Shader":{"info":{"id":"a","name":"A","username":"u"},"renderpass":[{"inputs":[],"code":"void mainImage(out vec4 o, in vec2 p){}","type":"image"}]}}"#,
        )
        .unwrap();
        assert!(!listing.usage.allows_commercial());
        let mit = shadertoy::parse(
            r#"{"Shader":{"info":{"id":"b","name":"B","username":"u"},"renderpass":[{"inputs":[],"code":"// The MIT License\n// Copyright\nvoid mainImage(out vec4 o, in vec2 p){}","type":"image"}]}}"#,
        )
        .unwrap();
        assert_eq!(mit.usage, Usage::Commercial("MIT".into()));
    }

    #[test]
    fn shadertoy_errors_are_reported() {
        let err = shadertoy::parse(r#"{"Error":"Invalid key"}"#).unwrap_err();
        assert_eq!(err.to_string(), "Invalid key");
    }

    #[test]
    fn query_encoding() {
        assert_eq!(
            shadertoy::encode_for_test("ray march é"),
            "ray%20march%20%C3%A9"
        );
    }

    /// Hits the live ISF site: `cargo test -p verso-online -- --ignored`.
    #[test]
    #[ignore]
    fn isf_live_search() {
        let (listings, total) = isf::search("kaleidoscope", isf::Sort::Popular, 0).unwrap();
        assert!(total > 0);
        assert!(listings.iter().all(|l| l.contents.is_some()));
        println!("{total} results, first: {}", listings[0].title);
    }
}
