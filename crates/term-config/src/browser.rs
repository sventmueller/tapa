//! The browser tabs (E22): where words typed into the address field are searched.

use serde::{Deserialize, Serialize};

/// The browser tabs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Browser {
    /// The engine that gets words typed into the address field.
    pub search_engine: SearchEngine,
}

/// A search engine the browser can send words to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    /// duckduckgo.com.
    #[default]
    DuckDuckGo,
    /// google.com.
    Google,
    /// bing.com.
    Bing,
    /// startpage.com.
    Startpage,
}

impl SearchEngine {
    /// Every engine, in the order the settings list them.
    pub const ALL: [SearchEngine; 4] = [
        SearchEngine::DuckDuckGo,
        SearchEngine::Google,
        SearchEngine::Bing,
        SearchEngine::Startpage,
    ];

    /// Its name, which no language translates.
    pub fn name(self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Google => "Google",
            SearchEngine::Bing => "Bing",
            SearchEngine::Startpage => "Startpage",
        }
    }

    /// The word the settings file uses.
    pub fn id(self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "duckduckgo",
            SearchEngine::Google => "google",
            SearchEngine::Bing => "bing",
            SearchEngine::Startpage => "startpage",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_section_reads_the_engines_word() {
        let (config, warnings) = crate::Config::parse("version = 1\n[browser]\nsearch_engine = \"startpage\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.browser.search_engine, SearchEngine::Startpage);
        let (plain, _) = crate::Config::parse("version = 1\n");
        assert_eq!(plain.browser.search_engine, SearchEngine::DuckDuckGo);
        for engine in SearchEngine::ALL {
            let text = format!("version = 1\n[browser]\nsearch_engine = \"{}\"\n", engine.id());
            assert_eq!(crate::Config::parse(&text).0.browser.search_engine, engine);
        }
    }
}
