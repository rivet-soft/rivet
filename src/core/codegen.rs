//! Code snippets for a request in popular languages and HTTP libraries (the
//! "Code" panel). Snippets are built from the resolved request (variables
//! substituted) plus its source, so multipart fields and file bodies stay
//! file references instead of inlined bytes.

use super::curl;
use super::types::{BodyKind, Environment, Method, RequestSpec, ResolvedRequest};
use super::vars;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeLang {
    Curl,
    Httpie,
    Wget,
    PythonRequests,
    JsFetch,
    NodeAxios,
    Go,
    RustReqwest,
    PhpCurl,
    CSharp,
    JavaOkHttp,
    Swift,
}

impl CodeLang {
    pub const ALL: [CodeLang; 12] = [
        Self::Curl,
        Self::Httpie,
        Self::Wget,
        Self::PythonRequests,
        Self::JsFetch,
        Self::NodeAxios,
        Self::Go,
        Self::RustReqwest,
        Self::PhpCurl,
        Self::CSharp,
        Self::JavaOkHttp,
        Self::Swift,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Curl => "cURL",
            Self::Httpie => "HTTPie",
            Self::Wget => "wget",
            Self::PythonRequests => "Python · requests",
            Self::JsFetch => "JavaScript · fetch",
            Self::NodeAxios => "Node.js · axios",
            Self::Go => "Go · net/http",
            Self::RustReqwest => "Rust · reqwest",
            Self::PhpCurl => "PHP · cURL",
            Self::CSharp => "C# · HttpClient",
            Self::JavaOkHttp => "Java · OkHttp",
            Self::Swift => "Swift · URLSession",
        }
    }

    /// Stable id (stored in settings).
    pub fn id(self) -> &'static str {
        match self {
            Self::Curl => "curl",
            Self::Httpie => "httpie",
            Self::Wget => "wget",
            Self::PythonRequests => "python-requests",
            Self::JsFetch => "js-fetch",
            Self::NodeAxios => "node-axios",
            Self::Go => "go",
            Self::RustReqwest => "rust-reqwest",
            Self::PhpCurl => "php-curl",
            Self::CSharp => "csharp",
            Self::JavaOkHttp => "java-okhttp",
            Self::Swift => "swift",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }

    /// Syntax highlighting the editor can apply (only bundled grammars).
    pub fn editor_language(self) -> Option<&'static str> {
        matches!(self, Self::JsFetch | Self::NodeAxios).then_some("javascript")
    }
}

/// One multipart field with variables substituted.
struct Part {
    name: String,
    value: String,
    file: bool,
}

/// What the snippet sends.
enum Payload {
    None,
    Text(String),
    Multipart(Vec<Part>),
    File(String),
}

struct Snippet<'a> {
    method: Method,
    url: &'a str,
    /// Without Content-Type for multipart (the library adds the boundary).
    headers: Vec<(&'a str, &'a str)>,
    content_type: Option<&'a str>,
    payload: Payload,
}

fn snippet<'a>(
    source: &RequestSpec,
    req: &'a ResolvedRequest,
    env: Option<&Environment>,
) -> Snippet<'a> {
    let sub = |s: &str| vars::substitute(s, env).0;
    let payload = if req.method == Method::Head {
        Payload::None
    } else {
        match source.body.kind {
            BodyKind::Multipart => Payload::Multipart(
                source
                    .body
                    .form
                    .iter()
                    .filter(|f| f.enabled && !f.key.trim().is_empty())
                    .map(|f| Part { name: sub(&f.key), value: sub(&f.value), file: f.file })
                    .collect(),
            ),
            BodyKind::Binary => match &source.body.file_path {
                Some(p) => Payload::File(sub(&p.to_string_lossy())),
                None => Payload::None,
            },
            _ => match &req.body {
                Some(b) if !b.is_empty() => Payload::Text(String::from_utf8_lossy(b).into_owned()),
                _ => Payload::None,
            },
        }
    };
    let multipart = matches!(payload, Payload::Multipart(_));
    let content_type = req
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str());
    let headers = req
        .headers
        .iter()
        .filter(|(k, _)| !(multipart && k.eq_ignore_ascii_case("content-type")))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    Snippet {
        method: req.method,
        url: &req.url,
        headers,
        content_type: if multipart { None } else { content_type },
        payload,
    }
}

/// File name part of a path, for multipart filenames.
fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or("file")
}

// ---- string literals ----------------------------------------------------------

/// A double-quoted literal with JSON escapes: valid in JavaScript, Python, Go,
/// C# and Java (`\uXXXX`, `\n`, `\"`, `\\`).
fn dq(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// POSIX shell single quotes.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// PHP single-quoted literal.
fn php(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Rust and Swift literals: `\u{…}` escapes instead of JSON's `\uXXXX`.
fn braced_unicode(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---- generators -----------------------------------------------------------------

/// The snippet for `lang`. `source` keeps file references for multipart and
/// binary bodies; `env` substitutes their variables.
pub fn generate(
    lang: CodeLang,
    source: &RequestSpec,
    req: &ResolvedRequest,
    env: Option<&Environment>,
) -> String {
    if lang == CodeLang::Curl {
        return curl::to_curl_with_source(source, req, env);
    }
    let s = snippet(source, req, env);
    match lang {
        CodeLang::Curl => unreachable!(),
        CodeLang::Httpie => httpie(&s),
        CodeLang::Wget => wget(&s),
        CodeLang::PythonRequests => python(&s),
        CodeLang::JsFetch => js_fetch(&s),
        CodeLang::NodeAxios => node_axios(&s),
        CodeLang::Go => go(&s),
        CodeLang::RustReqwest => rust(&s),
        CodeLang::PhpCurl => php_curl(&s),
        CodeLang::CSharp => csharp(&s),
        CodeLang::JavaOkHttp => java(&s),
        CodeLang::Swift => swift(&s),
    }
}

fn httpie(s: &Snippet) -> String {
    let mut parts = vec![format!("http --ignore-stdin {} {}", s.method.full_label(), sq(s.url))];
    if matches!(s.payload, Payload::Multipart(_)) {
        parts[0] = format!("http --ignore-stdin --multipart {} {}", s.method.full_label(), sq(s.url));
    }
    for (k, v) in &s.headers {
        parts.push(sq(&format!("{k}:{v}")));
    }
    match &s.payload {
        Payload::None => {}
        Payload::Text(t) => parts.push(format!("--raw {}", sq(t))),
        Payload::Multipart(fields) => {
            for f in fields {
                let sep = if f.file { "@" } else { "=" };
                parts.push(sq(&format!("{}{sep}{}", f.name, f.value)));
            }
        }
        Payload::File(p) => parts.push(format!("< {}", sq(p))),
    }
    parts.join(" \\\n  ")
}

fn wget(s: &Snippet) -> String {
    let mut parts = vec![format!("wget --quiet --method={}", s.method.full_label())];
    for (k, v) in &s.headers {
        parts.push(format!("--header={}", sq(&format!("{k}: {v}"))));
    }
    let mut note = None;
    match &s.payload {
        Payload::None => {}
        Payload::Text(t) => parts.push(format!("--body-data={}", sq(t))),
        Payload::File(p) => parts.push(format!("--body-file={}", sq(p))),
        Payload::Multipart(_) => {
            note = Some("# wget не умеет multipart/form-data: используйте cURL или HTTPie\n");
        }
    }
    parts.push("--output-document -".into());
    parts.push(sq(s.url));
    format!("{}{}", note.unwrap_or(""), parts.join(" \\\n  "))
}

fn python(s: &Snippet) -> String {
    let mut out = String::from("import requests\n\n");
    out += &format!("url = {}\n", dq(s.url));
    out += "headers = {\n";
    for (k, v) in &s.headers {
        out += &format!("    {}: {},\n", dq(k), dq(v));
    }
    out += "}\n";
    let mut args = String::from("url, headers=headers");
    match &s.payload {
        Payload::None => {}
        Payload::Text(t) => {
            out += &format!("payload = {}\n", dq(t));
            args += ", data=payload.encode(\"utf-8\")";
        }
        Payload::File(p) => {
            out += &format!("with open({}, \"rb\") as f:\n    payload = f.read()\n", dq(p));
            args += ", data=payload";
        }
        Payload::Multipart(fields) => {
            out += "data = [\n";
            for f in fields.iter().filter(|f| !f.file) {
                out += &format!("    ({}, {}),\n", dq(&f.name), dq(&f.value));
            }
            out += "]\nfiles = [\n";
            for f in fields.iter().filter(|f| f.file) {
                out += &format!(
                    "    ({}, ({}, open({}, \"rb\"))),\n",
                    dq(&f.name),
                    dq(file_name(&f.value)),
                    dq(&f.value)
                );
            }
            out += "]\n";
            args += ", data=data, files=files";
        }
    }
    out += &format!(
        "\nresponse = requests.request({}, {args})\n\nprint(response.status_code)\nprint(response.text)\n",
        dq(s.method.full_label())
    );
    out
}

fn js_object(headers: &[(&str, &str)], indent: &str) -> String {
    if headers.is_empty() {
        return "{}".into();
    }
    let mut out = String::from("{\n");
    for (k, v) in headers {
        out += &format!("{indent}  {}: {},\n", dq(k), dq(v));
    }
    out += &format!("{indent}}}");
    out
}

fn js_fetch(s: &Snippet) -> String {
    let mut pre = String::new();
    let body = match &s.payload {
        Payload::None => None,
        Payload::Text(t) => Some(dq(t)),
        Payload::File(p) => {
            pre += "import { readFile } from \"node:fs/promises\";\n\n";
            Some(format!("await readFile({})", dq(p)))
        }
        Payload::Multipart(fields) => {
            if fields.iter().any(|f| f.file) {
                pre += "import { readFile } from \"node:fs/promises\";\n\n";
            }
            pre += "const form = new FormData();\n";
            for f in fields {
                if f.file {
                    pre += &format!(
                        "form.append({}, new Blob([await readFile({})]), {});\n",
                        dq(&f.name),
                        dq(&f.value),
                        dq(file_name(&f.value))
                    );
                } else {
                    pre += &format!("form.append({}, {});\n", dq(&f.name), dq(&f.value));
                }
            }
            pre += "\n";
            Some("form".into())
        }
    };
    let mut out = pre;
    out += &format!("const response = await fetch({}, {{\n", dq(s.url));
    out += &format!("  method: {},\n", dq(s.method.full_label()));
    out += &format!("  headers: {},\n", js_object(&s.headers, "  "));
    if let Some(b) = body {
        out += &format!("  body: {b},\n");
    }
    out += "});\n\nconsole.log(response.status);\nconsole.log(await response.text());\n";
    out
}

fn node_axios(s: &Snippet) -> String {
    let mut out = String::from("import axios from \"axios\";\n");
    let mut pre = String::new();
    let mut headers = js_object(&s.headers, "  ");
    let data = match &s.payload {
        Payload::None => None,
        Payload::Text(t) => Some(dq(t)),
        Payload::File(p) => {
            out += "import fs from \"node:fs\";\n";
            Some(format!("fs.readFileSync({})", dq(p)))
        }
        Payload::Multipart(fields) => {
            out += "import FormData from \"form-data\";\n";
            if fields.iter().any(|f| f.file) {
                out += "import fs from \"node:fs\";\n";
            }
            pre += "const form = new FormData();\n";
            for f in fields {
                if f.file {
                    pre += &format!("form.append({}, fs.createReadStream({}));\n", dq(&f.name), dq(&f.value));
                } else {
                    pre += &format!("form.append({}, {});\n", dq(&f.name), dq(&f.value));
                }
            }
            pre += "\n";
            let mut merged = String::from("{\n    ...form.getHeaders(),\n");
            for (k, v) in &s.headers {
                merged += &format!("    {}: {},\n", dq(k), dq(v));
            }
            merged += "  }";
            headers = merged;
            Some("form".into())
        }
    };
    out += "\n";
    out += &pre;
    out += "const response = await axios.request({\n";
    out += &format!("  method: {},\n", dq(&s.method.full_label().to_ascii_lowercase()));
    out += &format!("  url: {},\n", dq(s.url));
    out += &format!("  headers: {headers},\n");
    if let Some(d) = data {
        out += &format!("  data: {d},\n");
    }
    out += "  // Не бросать исключение на 4xx/5xx — как в Rivet.\n  validateStatus: () => true,\n";
    out += "});\n\nconsole.log(response.status);\nconsole.log(response.data);\n";
    out
}

fn go(s: &Snippet) -> String {
    let mut imports = vec!["\"fmt\"", "\"io\"", "\"net/http\""];
    let mut body_setup = String::new();
    let body_var = match &s.payload {
        Payload::None => "nil".to_string(),
        Payload::Text(t) => {
            imports.push("\"strings\"");
            body_setup += &format!("\tbody := strings.NewReader({})\n", dq(t));
            "body".into()
        }
        Payload::File(p) => {
            imports.push("\"os\"");
            body_setup += &format!(
                "\tbody, err := os.Open({})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer body.Close()\n",
                dq(p)
            );
            "body".into()
        }
        Payload::Multipart(fields) => {
            imports.extend(["\"bytes\"", "\"mime/multipart\""]);
            if fields.iter().any(|f| f.file) {
                imports.extend(["\"os\"", "\"path/filepath\""]);
            }
            body_setup += "\tbody := &bytes.Buffer{}\n\twriter := multipart.NewWriter(body)\n";
            for f in fields {
                if f.file {
                    body_setup += &format!(
                        "\tif file, err := os.Open({path}); err == nil {{\n\t\tpart, _ := writer.CreateFormFile({name}, filepath.Base({path}))\n\t\tio.Copy(part, file)\n\t\tfile.Close()\n\t}} else {{\n\t\tpanic(err)\n\t}}\n",
                        path = dq(&f.value),
                        name = dq(&f.name)
                    );
                } else {
                    body_setup += &format!("\twriter.WriteField({}, {})\n", dq(&f.name), dq(&f.value));
                }
            }
            body_setup += "\twriter.Close()\n";
            "body".into()
        }
    };
    imports.sort();
    imports.dedup();
    let mut out = String::from("package main\n\nimport (\n");
    for i in &imports {
        out += &format!("\t{i}\n");
    }
    out += ")\n\nfunc main() {\n";
    out += &body_setup;
    // `req` is new in every branch, so `:=` also works after `body, err := os.Open`.
    out += &format!(
        "\treq, err := http.NewRequest({}, {}, {body_var})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n",
        dq(s.method.full_label()),
        dq(s.url)
    );
    for (k, v) in &s.headers {
        out += &format!("\treq.Header.Add({}, {})\n", dq(k), dq(v));
    }
    if matches!(s.payload, Payload::Multipart(_)) {
        out += "\treq.Header.Set(\"Content-Type\", writer.FormDataContentType())\n";
    }
    out += "\n\tres, err := http.DefaultClient.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer res.Body.Close()\n\n\tdata, err := io.ReadAll(res.Body)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tfmt.Println(res.StatusCode)\n\tfmt.Println(string(data))\n}\n";
    out
}

fn rust(s: &Snippet) -> String {
    let method = match s.method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Patch => "PATCH",
        Method::Delete => "DELETE",
        Method::Head => "HEAD",
        Method::Options => "OPTIONS",
    };
    let mut out = String::from(
        "// Cargo.toml: reqwest = { version = \"0.12\", features = [\"blocking\", \"multipart\"] }\n\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n",
    );
    if let Payload::Multipart(fields) = &s.payload {
        out += "    let form = reqwest::blocking::multipart::Form::new()";
        for f in fields {
            if f.file {
                out += &format!("\n        .file({}, {})?", braced_unicode(&f.name), braced_unicode(&f.value));
            } else {
                out += &format!("\n        .text({}, {})", braced_unicode(&f.name), braced_unicode(&f.value));
            }
        }
        out += ";\n\n";
    }
    out += "    let client = reqwest::blocking::Client::new();\n    let response = client\n";
    out += &format!("        .request(reqwest::Method::{method}, {})\n", braced_unicode(s.url));
    for (k, v) in &s.headers {
        out += &format!("        .header({}, {})\n", braced_unicode(k), braced_unicode(v));
    }
    match &s.payload {
        Payload::None => {}
        Payload::Text(t) => out += &format!("        .body({})\n", braced_unicode(t)),
        Payload::File(p) => out += &format!("        .body(std::fs::read({})?)\n", braced_unicode(p)),
        Payload::Multipart(_) => out += "        .multipart(form)\n",
    }
    out += "        .send()?;\n\n    println!(\"{}\", response.status());\n    println!(\"{}\", response.text()?);\n    Ok(())\n}\n";
    out
}

fn php_curl(s: &Snippet) -> String {
    let mut out = String::from("<?php\n\n$curl = curl_init();\n\ncurl_setopt_array($curl, [\n");
    out += &format!("    CURLOPT_URL => {},\n", php(s.url));
    out += "    CURLOPT_RETURNTRANSFER => true,\n";
    out += &format!("    CURLOPT_CUSTOMREQUEST => {},\n", php(s.method.full_label()));
    if s.method == Method::Head {
        out += "    CURLOPT_NOBODY => true,\n";
    }
    if !s.headers.is_empty() {
        out += "    CURLOPT_HTTPHEADER => [\n";
        for (k, v) in &s.headers {
            out += &format!("        {},\n", php(&format!("{k}: {v}")));
        }
        out += "    ],\n";
    }
    match &s.payload {
        Payload::None => {}
        Payload::Text(t) => out += &format!("    CURLOPT_POSTFIELDS => {},\n", php(t)),
        Payload::File(p) => out += &format!("    CURLOPT_POSTFIELDS => file_get_contents({}),\n", php(p)),
        Payload::Multipart(fields) => {
            out += "    CURLOPT_POSTFIELDS => [\n";
            for f in fields {
                if f.file {
                    out += &format!("        {} => new CURLFile({}),\n", php(&f.name), php(&f.value));
                } else {
                    out += &format!("        {} => {},\n", php(&f.name), php(&f.value));
                }
            }
            out += "    ],\n";
        }
    }
    out += "]);\n\n$response = curl_exec($curl);\n$status = curl_getinfo($curl, CURLINFO_RESPONSE_CODE);\ncurl_close($curl);\n\necho $status . PHP_EOL;\necho $response;\n";
    out
}

fn csharp(s: &Snippet) -> String {
    let mut out = String::from(
        "using System.Net.Http.Headers;\n\nusing var client = new HttpClient();\n",
    );
    out += &format!(
        "using var request = new HttpRequestMessage(new HttpMethod({}), {});\n",
        dq(s.method.full_label()),
        dq(s.url)
    );
    // Content headers live on the content, not on the request.
    let content_header = |k: &str| {
        matches!(
            k.to_ascii_lowercase().as_str(),
            "content-type" | "content-length" | "content-encoding" | "content-language" | "content-disposition"
        )
    };
    for (k, v) in s.headers.iter().filter(|(k, _)| !content_header(k)) {
        out += &format!("request.Headers.TryAddWithoutValidation({}, {});\n", dq(k), dq(v));
    }
    match &s.payload {
        Payload::None => {}
        Payload::Text(t) => out += &format!("request.Content = new StringContent({});\n", dq(t)),
        Payload::File(p) => out += &format!("request.Content = new ByteArrayContent(File.ReadAllBytes({}));\n", dq(p)),
        Payload::Multipart(fields) => {
            out += "var form = new MultipartFormDataContent();\n";
            for f in fields {
                if f.file {
                    out += &format!(
                        "form.Add(new StreamContent(File.OpenRead({})), {}, {});\n",
                        dq(&f.value),
                        dq(&f.name),
                        dq(file_name(&f.value))
                    );
                } else {
                    out += &format!("form.Add(new StringContent({}), {});\n", dq(&f.value), dq(&f.name));
                }
            }
            out += "request.Content = form;\n";
        }
    }
    if !matches!(s.payload, Payload::None | Payload::Multipart(_)) {
        if let Some(ct) = s.content_type {
            out += &format!("request.Content.Headers.ContentType = MediaTypeHeaderValue.Parse({});\n", dq(ct));
        }
    }
    out += "\nusing var response = await client.SendAsync(request);\nConsole.WriteLine((int)response.StatusCode);\nConsole.WriteLine(await response.Content.ReadAsStringAsync());\n";
    out
}

fn java(s: &Snippet) -> String {
    let mut out = String::from("import java.io.File;\nimport okhttp3.*;\n\nOkHttpClient client = new OkHttpClient();\n");
    let media = s.content_type.map_or("null".to_string(), |ct| format!("MediaType.parse({})", dq(ct)));
    let needs_body = matches!(s.method, Method::Post | Method::Put | Method::Patch);
    let body = match &s.payload {
        Payload::None if needs_body => "RequestBody.create(new byte[0], null)".to_string(),
        Payload::None => "null".to_string(),
        Payload::Text(t) => format!("RequestBody.create({}, {media})", dq(t)),
        Payload::File(p) => format!("RequestBody.create(new File({}), {media})", dq(p)),
        Payload::Multipart(fields) => {
            let mut b = String::from("new MultipartBody.Builder()\n    .setType(MultipartBody.FORM)");
            for f in fields {
                if f.file {
                    b += &format!(
                        "\n    .addFormDataPart({}, {}, RequestBody.create(new File({}), MediaType.parse(\"application/octet-stream\")))",
                        dq(&f.name),
                        dq(file_name(&f.value)),
                        dq(&f.value)
                    );
                } else {
                    b += &format!("\n    .addFormDataPart({}, {})", dq(&f.name), dq(&f.value));
                }
            }
            b + "\n    .build()"
        }
    };
    out += &format!("RequestBody body = {body};\n");
    out += &format!("Request request = new Request.Builder()\n    .url({})\n    .method({}, body)\n", dq(s.url), dq(s.method.full_label()));
    for (k, v) in &s.headers {
        out += &format!("    .addHeader({}, {})\n", dq(k), dq(v));
    }
    out += "    .build();\n\ntry (Response response = client.newCall(request).execute()) {\n    System.out.println(response.code());\n    System.out.println(response.body().string());\n}\n";
    out
}

fn swift(s: &Snippet) -> String {
    let mut out = String::from("import Foundation\n\n");
    out += &format!("var request = URLRequest(url: URL(string: {})!)\n", braced_unicode(s.url));
    out += &format!("request.httpMethod = {}\n", braced_unicode(s.method.full_label()));
    for (k, v) in &s.headers {
        out += &format!("request.setValue({}, forHTTPHeaderField: {})\n", braced_unicode(v), braced_unicode(k));
    }
    match &s.payload {
        Payload::None => {}
        Payload::Text(t) => out += &format!("request.httpBody = Data({}.utf8)\n", braced_unicode(t)),
        Payload::File(p) => out += &format!("request.httpBody = try Data(contentsOf: URL(fileURLWithPath: {}))\n", braced_unicode(p)),
        Payload::Multipart(fields) => {
            out += "let boundary = \"Boundary-\\(UUID().uuidString)\"\nrequest.setValue(\"multipart/form-data; boundary=\\(boundary)\", forHTTPHeaderField: \"Content-Type\")\nvar body = Data()\n";
            for f in fields {
                if f.file {
                    out += &format!(
                        "body.append(Data(\"--\\(boundary)\\r\\nContent-Disposition: form-data; name=\\\"\\({name})\\\"; filename=\\\"\\({file})\\\"\\r\\nContent-Type: application/octet-stream\\r\\n\\r\\n\".utf8))\nbody.append(try Data(contentsOf: URL(fileURLWithPath: {path})))\nbody.append(Data(\"\\r\\n\".utf8))\n",
                        name = braced_unicode(&f.name),
                        file = braced_unicode(file_name(&f.value)),
                        path = braced_unicode(&f.value)
                    );
                } else {
                    out += &format!(
                        "body.append(Data(\"--\\(boundary)\\r\\nContent-Disposition: form-data; name=\\\"\\({name})\\\"\\r\\n\\r\\n\\({value})\\r\\n\".utf8))\n",
                        name = braced_unicode(&f.name),
                        value = braced_unicode(&f.value)
                    );
                }
            }
            out += "body.append(Data(\"--\\(boundary)--\\r\\n\".utf8))\nrequest.httpBody = body\n";
        }
    }
    out += "\nlet (data, response) = try await URLSession.shared.data(for: request)\nprint((response as! HTTPURLResponse).statusCode)\nprint(String(decoding: data, as: UTF8.self))\n";
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Body, KeyValue};

    fn json_post() -> (RequestSpec, ResolvedRequest) {
        let mut r = RequestSpec::new("Create", Method::Post, "https://api.x.com/users?q=a b");
        r.headers = vec![KeyValue::new("X-Note", "it's \"quoted\"")];
        r.body = Body::json("{\"name\": \"Ann\\n\", \"city\": \"Тверь\"}");
        let resolved = vars::resolve(&r, None).unwrap();
        (r, resolved)
    }

    fn multipart() -> (RequestSpec, ResolvedRequest) {
        let mut r = RequestSpec::new("Upload", Method::Post, "https://x/upload");
        let mut file = KeyValue::new("file", "/tmp/a b.png");
        file.file = true;
        r.body = Body { kind: BodyKind::Multipart, form: vec![KeyValue::new("note", "hi"), file], ..Body::default() };
        // Resolving a multipart body reads the file; build the resolved request by hand.
        let resolved = ResolvedRequest {
            method: Method::Post,
            url: "https://x/upload".into(),
            headers: vec![("Content-Type".into(), "multipart/form-data; boundary=x".into())],
            body: Some(b"--x--".to_vec()),
            missing_vars: vec![],
            omit_default_headers: Vec::new(),
        };
        (r, resolved)
    }

    #[test]
    fn every_language_renders_method_url_headers_and_body() {
        let (r, res) = json_post();
        for lang in CodeLang::ALL {
            let code = generate(lang, &r, &res, None);
            assert!(code.contains("api.x.com/users"), "{lang:?}: {code}");
            assert!(code.contains("POST") || code.contains("post"), "{lang:?}");
            assert!(code.contains("X-Note"), "{lang:?}");
            assert!(code.contains("Content-Type") || lang == CodeLang::CSharp, "{lang:?}");
            assert!(code.contains("Ann"), "{lang:?}: {code}");
            assert!(code.contains("Тверь") || code.contains("\\u0422"), "{lang:?}");
            assert_eq!(CodeLang::from_id(lang.id()), Some(lang));
        }
        // C# puts Content-Type on the content.
        assert!(generate(CodeLang::CSharp, &r, &res, None).contains("MediaTypeHeaderValue.Parse(\"application/json\")"));
    }

    #[test]
    fn escaping_per_language() {
        let (r, res) = json_post();
        let py = generate(CodeLang::PythonRequests, &r, &res, None);
        assert!(py.contains(r#""X-Note": "it's \"quoted\"""#), "{py}");
        assert!(py.contains(r#"payload = "{\"name\": \"Ann\\n\", \"city\": \"Тверь\"}""#), "{py}");
        let sh = generate(CodeLang::Httpie, &r, &res, None);
        assert!(sh.contains(r#"'X-Note:it'\''s "quoted"'"#), "{sh}");
        let php_code = generate(CodeLang::PhpCurl, &r, &res, None);
        assert!(php_code.contains(r#"'X-Note: it\'s "quoted"'"#), "{php_code}");
        let rs = generate(CodeLang::RustReqwest, &r, &res, None);
        assert!(rs.contains(r#".header("X-Note", "it's \"quoted\"")"#), "{rs}");
        assert!(rs.contains("reqwest::Method::POST"));
        assert_eq!(braced_unicode("a\u{1}b"), "\"a\\u{1}b\"");
        assert_eq!(dq("a\u{1}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn multipart_and_files_stay_references() {
        let (r, res) = multipart();
        for lang in CodeLang::ALL {
            let code = generate(lang, &r, &res, None);
            assert!(!code.contains("boundary=x"), "{lang:?} must not keep the resolved boundary: {code}");
            if lang != CodeLang::Wget {
                assert!(code.contains("/tmp/a b.png"), "{lang:?}: {code}");
                assert!(code.contains("note"), "{lang:?}");
            }
        }
        assert!(generate(CodeLang::Wget, &r, &res, None).starts_with("# wget не умеет multipart"));
        let py = generate(CodeLang::PythonRequests, &r, &res, None);
        assert!(py.contains(r#"("file", ("a b.png", open("/tmp/a b.png", "rb")))"#), "{py}");
        assert!(generate(CodeLang::Go, &r, &res, None).contains("writer.FormDataContentType()"));

        let mut bin = RequestSpec::new("Bin", Method::Put, "https://x/b");
        bin.body = Body { kind: BodyKind::Binary, file_path: Some("{{dir}}/data.bin".into()), ..Body::default() };
        let mut env = Environment::new("e", Default::default());
        env.vars = vec![KeyValue::new("dir", "/srv")];
        let res = ResolvedRequest { method: Method::Put, url: "https://x/b".into(), headers: vec![], body: Some(vec![0, 1]), missing_vars: vec![], omit_default_headers: Vec::new() };
        assert!(generate(CodeLang::NodeAxios, &bin, &res, Some(&env)).contains("fs.readFileSync(\"/srv/data.bin\")"));
        assert!(generate(CodeLang::Swift, &bin, &res, Some(&env)).contains("URL(fileURLWithPath: \"/srv/data.bin\")"));
        // `body, err := os.Open(..)` comes first: `req` still needs `:=`.
        let go_bin = generate(CodeLang::Go, &bin, &res, Some(&env));
        assert!(go_bin.contains("body, err := os.Open(\"/srv/data.bin\")"), "{go_bin}");
        assert!(go_bin.contains("req, err := http.NewRequest(\"PUT\""), "{go_bin}");
    }

    #[test]
    fn get_and_head_have_no_body() {
        let r = RequestSpec::new("Get", Method::Get, "https://x/");
        let res = vars::resolve(&r, None).unwrap();
        let java = generate(CodeLang::JavaOkHttp, &r, &res, None);
        assert!(java.contains("RequestBody body = null;"), "{java}");
        let go_code = generate(CodeLang::Go, &r, &res, None);
        assert!(go_code.contains("http.NewRequest(\"GET\", \"https://x/\", nil)"), "{go_code}");
        assert!(!go_code.contains("\"strings\""));
        let mut post = RequestSpec::new("P", Method::Post, "https://x/");
        post.body = Body::default();
        let res = vars::resolve(&post, None).unwrap();
        assert!(generate(CodeLang::JavaOkHttp, &post, &res, None).contains("RequestBody.create(new byte[0], null)"));
        let head = RequestSpec::new("H", Method::Head, "https://x/");
        let res = vars::resolve(&head, None).unwrap();
        assert!(generate(CodeLang::PhpCurl, &head, &res, None).contains("CURLOPT_NOBODY => true"));
    }
}
