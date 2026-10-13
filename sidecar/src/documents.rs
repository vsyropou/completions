use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct Document {
    pub text: String,
    pub version: i64,
    pub language: String,
}

#[derive(Clone, Default)]
pub struct Documents {
    inner: Arc<RwLock<HashMap<String, Document>>>,
}

impl Documents {
    pub async fn did_open(&self, params: &serde_json::Value) {
        let Some(uri) = params
            .get("uri")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let Some(text) = params
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let Some(version) = params.get("version").and_then(|v| v.as_i64()) else {
            return;
        };
        let language = params
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("typescript")
            .to_owned();
        let mut m = self.inner.write().await;
        m.insert(
            uri,
            Document {
                text,
                version,
                language,
            },
        );
    }

    pub async fn did_change(&self, params: &serde_json::Value) {
        let Some(uri) = params
            .get("uri")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let Some(text) = params
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let Some(version) = params.get("version").and_then(|v| v.as_i64()) else {
            return;
        };
        let mut m = self.inner.write().await;
        match m.get(&uri) {
            Some(d) if version > d.version => {
                let lang = d.language.clone();
                m.insert(
                    uri,
                    Document {
                        text,
                        version,
                        language: lang,
                    },
                );
            }
            Some(d) => {
                tracing::warn!(uri = %uri, requested = version, current = d.version, "stale didChange, discarded");
            }
            None => {
                tracing::warn!(uri = %uri, "didChange for unopened doc, inserting");
                m.insert(
                    uri,
                    Document {
                        text,
                        version,
                        language: "typescript".into(),
                    },
                );
            }
        }
    }

    pub async fn did_close(&self, params: &serde_json::Value) {
        if let Some(uri) = params.get("uri").and_then(|v| v.as_str()) {
            let mut m = self.inner.write().await;
            m.remove(uri);
        }
    }

    pub async fn get(&self, uri: &str) -> Option<Document> {
        self.inner.read().await.get(uri).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open(docs: &Documents, uri: &str, text: &str, version: i64) {
        docs.did_open(&serde_json::json!({
            "uri": uri, "text": text, "version": version, "language": "typescript"
        }))
        .await;
    }

    async fn change(docs: &Documents, uri: &str, text: &str, version: i64) {
        docs.did_change(&serde_json::json!({ "uri": uri, "text": text, "version": version }))
            .await;
    }

    #[tokio::test]
    async fn version_must_increase_monotonically() {
        let docs = Documents::default();
        open(&docs, "u", "a", 3).await;
        change(&docs, "u", "b", 4).await;
        assert_eq!(docs.get("u").await.unwrap().text, "b");

        // equal: rejected
        change(&docs, "u", "c", 4).await;
        assert_eq!(docs.get("u").await.unwrap().text, "b");
        assert_eq!(docs.get("u").await.unwrap().version, 4);

        // lower: rejected
        change(&docs, "u", "d", 2).await;
        assert_eq!(docs.get("u").await.unwrap().text, "b");

        // higher: accepted
        change(&docs, "u", "e", 7).await;
        assert_eq!(docs.get("u").await.unwrap().text, "e");
        assert_eq!(docs.get("u").await.unwrap().version, 7);
    }
}
