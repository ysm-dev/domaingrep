use domaingrep::hack::HackTrie;

#[test]
fn detects_basic_domain_hacks() {
    let trie = HackTrie::new(["sh", "shop", "show"]);
    let matches = trie.find_matches("bunsh");
    let domains = matches
        .into_iter()
        .map(|item| item.domain())
        .collect::<Vec<_>>();

    assert_eq!(domains, vec!["bun.sh"]);
}

#[test]
fn sorts_longest_matching_tld_first() {
    let trie = HackTrie::new(["i", "nai"]);
    let matches = trie.find_matches("openai");
    let domains = matches
        .into_iter()
        .map(|item| item.domain())
        .collect::<Vec<_>>();

    assert_eq!(domains, vec!["ope.nai", "opena.i"]);
}
