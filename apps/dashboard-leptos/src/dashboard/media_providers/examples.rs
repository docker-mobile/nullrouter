pub(super) fn example_path(kind_id: &str) -> Option<&'static str> {
    match kind_id {
        "embedding" => Some("/v1/embeddings"),
        "webSearch" => Some("/v1/search"),
        "webFetch" => Some("/v1/web/fetch"),
        "image" => Some("/v1/images/generations"),
        "tts" => Some("/v1/audio/speech"),
        _ => None,
    }
}

pub(super) fn example_body(kind_id: &str) -> Option<&'static str> {
    match kind_id {
        "embedding" => Some(r#"{"model":"embedding_combo","input":"Hello from nullrouter"}"#),
        "webSearch" => Some(
            r#"{"model":"search-combo","query":"What is the latest news about AI?","search_type":"web","max_results":5}"#,
        ),
        "webFetch" => {
            Some(r#"{"model":"fetch-combo","url":"https://example.com","format":"markdown"}"#)
        }
        "image" => Some(
            r#"{"model":"image-combo","prompt":"A cute cat playing piano","n":1,"size":"1024x1024"}"#,
        ),
        "tts" => Some(r#"{"model":"tts-combo","input":"Hello, this is a test.","voice":"alloy"}"#),
        _ => None,
    }
}

pub(super) fn curl_preview(
    path: Option<&'static str>,
    body: Option<&'static str>,
    name: &str,
) -> String {
    match (path, body) {
        (Some(path), Some(body)) => {
            let scoped_body = body
                .replace("embedding_combo", name)
                .replace("search-combo", name)
                .replace("fetch-combo", name)
                .replace("image-combo", name)
                .replace("tts-combo", name);
            format!(
                "curl -X POST http://localhost:20128{path} \\\n  -H \"Content-Type: application/json\" \\\n  -H \"Authorization: Bearer YOUR_KEY\" \\\n  -d '{scoped_body}'"
            )
        }
        _ => "No executable example is available for this combo kind yet.".to_owned(),
    }
}
