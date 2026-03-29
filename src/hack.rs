use crate::input::validate_label;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HackMatch {
    pub sld: String,
    pub tld: String,
}

impl HackMatch {
    pub fn domain(&self) -> String {
        format!("{}.{}", self.sld, self.tld)
    }
}

#[derive(Debug, Default)]
struct TrieNode {
    terminal: bool,
    children: HashMap<char, TrieNode>,
}

#[derive(Debug, Default)]
pub struct HackTrie {
    root: TrieNode,
}

impl HackTrie {
    pub fn new<'a>(tlds: impl IntoIterator<Item = &'a str>) -> Self {
        let mut trie = Self::default();
        for tld in tlds {
            trie.insert(tld);
        }
        trie
    }

    pub fn insert(&mut self, tld: &str) {
        let mut current = &mut self.root;
        for ch in tld.chars().rev() {
            current = current.children.entry(ch).or_default();
        }
        current.terminal = true;
    }

    pub fn find_matches(&self, input: &str) -> Vec<HackMatch> {
        let mut current = &self.root;
        let mut matches = Vec::new();

        for (depth, ch) in input.chars().rev().enumerate() {
            let Some(next) = current.children.get(&ch) else {
                break;
            };
            current = next;

            if current.terminal {
                let tld_len = depth + 1;
                if input.len() > tld_len {
                    let split_at = input.len() - tld_len;
                    let sld = &input[..split_at];
                    let tld = &input[split_at..];

                    if validate_label(sld).is_ok() {
                        matches.push(HackMatch {
                            sld: sld.to_string(),
                            tld: tld.to_string(),
                        });
                    }
                }
            }
        }

        matches.sort_by(|left, right| left.sld.len().cmp(&right.sld.len()));
        matches
    }
}
