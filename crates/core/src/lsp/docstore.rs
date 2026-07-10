//! Document store for LSP server: tracks open documents with version and text.
//! Full-sync changes replace text wholesale. Sorted URI iteration for deterministic
//! republish sweeps.

use std::collections::HashMap;

/// A document with its current version and text content.
pub struct Document {
    pub version: i32,
    pub text: String,
}

/// Store of open documents indexed by URI.
#[derive(Default)]
pub struct DocStore {
    docs: HashMap<String, Document>,
}

impl DocStore {
    /// Create a new empty document store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a document with the given URI, version, and text.
    pub fn open(&mut self, uri: &str, version: i32, text: String) {
        self.docs
            .insert(uri.to_string(), Document { version, text });
    }

    /// Change a document by full-sync replacement: update version and text.
    /// No-op if the URI is not currently open (defensive against client bugs).
    pub fn change(&mut self, uri: &str, version: i32, text: String) {
        if let Some(doc) = self.docs.get_mut(uri) {
            doc.version = version;
            doc.text = text;
        }
    }

    /// Close a document: remove it from the store.
    pub fn close(&mut self, uri: &str) {
        self.docs.remove(uri);
    }

    /// Get a reference to an open document by URI.
    pub fn get(&self, uri: &str) -> Option<&Document> {
        self.docs.get(uri)
    }

    /// Get all open URIs in sorted order for deterministic republish sweeps.
    pub fn uris(&self) -> Vec<String> {
        let mut uris: Vec<_> = self.docs.keys().cloned().collect();
        uris.sort();
        uris
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_and_get() {
        let mut store = DocStore::new();
        store.open("file:///a.fake", 1, "let x = 5;".to_string());

        let doc = store.get("file:///a.fake");
        assert!(doc.is_some());
        let doc = doc.unwrap();
        assert_eq!(doc.version, 1);
        assert_eq!(doc.text, "let x = 5;");
    }

    #[test]
    fn test_change_replaces_text_and_version() {
        let mut store = DocStore::new();
        store.open("file:///a.fake", 1, "old".to_string());
        store.change("file:///a.fake", 2, "new".to_string());

        let doc = store.get("file:///a.fake").unwrap();
        assert_eq!(doc.version, 2);
        assert_eq!(doc.text, "new");
    }

    #[test]
    fn test_close_removes_document() {
        let mut store = DocStore::new();
        store.open("file:///a.fake", 1, "text".to_string());
        assert!(store.get("file:///a.fake").is_some());

        store.close("file:///a.fake");
        assert!(store.get("file:///a.fake").is_none());
    }

    #[test]
    fn test_uris_sorted() {
        let mut store = DocStore::new();
        store.open("file:///c.fake", 1, "".to_string());
        store.open("file:///a.fake", 1, "".to_string());
        store.open("file:///b.fake", 1, "".to_string());

        let uris = store.uris();
        assert_eq!(
            uris,
            vec!["file:///a.fake", "file:///b.fake", "file:///c.fake"]
        );
    }

    #[test]
    fn test_change_on_unknown_uri_is_noop() {
        let mut store = DocStore::new();
        store.open("file:///a.fake", 1, "text".to_string());

        // Change on unknown URI should not panic and should be a no-op
        store.change("file:///unknown.fake", 99, "new".to_string());

        // Original document unchanged
        assert_eq!(store.get("file:///a.fake").unwrap().version, 1);
        assert_eq!(store.get("file:///a.fake").unwrap().text, "text");

        // Unknown URI still not in store
        assert!(store.get("file:///unknown.fake").is_none());
    }
}
