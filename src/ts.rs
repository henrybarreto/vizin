//! Tree-sitter based parsing for the decompiler view.
//! Provides bracket matching, symbol resolution, word extraction,
//! scope detection, and code-only search that correctly handles
//! strings, comments, and nested constructs.


use tree_sitter::{Node, Parser, Tree};

impl Default for TsParser {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TsParser {
    parser: Parser,
    /// Memoized (source, tree) pair. Decompiled functions can be thousands of
    /// lines; re-parsing on every cursor move/key action would be wasteful
    /// since the code is almost always unchanged between calls. `Tree::clone`
    /// is a cheap refcount bump, so caching the parsed tree (not just reusing
    /// it for incremental re-parse) is the correct way to reuse it here.
    cache: Option<(String, Tree)>,
}

impl TsParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        let language: tree_sitter::Language = tree_sitter_c::LANGUAGE.into();
        // Can only fail on a tree-sitter/grammar version mismatch, a build-time
        // invariant this crate's Cargo.toml pins — not a runtime condition callers
        // can recover from, and `Default`/`new()` can't return `Result`.
        #[allow(clippy::expect_used)]
        parser.set_language(&language).expect("tree-sitter-c grammar version mismatch");
        Self { parser, cache: None }
    }

    pub fn parse(&mut self, code: &str) -> Option<Tree> {
        if let Some((cached_code, tree)) = &self.cache {
            if cached_code == code {
                return Some(tree.clone());
            }
        }
        let tree = self.parser.parse(code, None)?;
        self.cache = Some((code.to_string(), tree.clone()));
        Some(tree)
    }

    // -- Bracket matching --------------------------------------------------

    /// Find the matching bracket for byte position `pos`.
    pub fn find_match(&mut self, code: &str, pos: usize) -> Option<usize> {
        let tree = self.parse(code)?;
        let root = tree.root_node();
        let byte = *code.as_bytes().get(pos)?;
        let (open, close) = match byte {
            b'{' => (b'{', b'}'),
            b'(' => (b'(', b')'),
            b'[' => (b'[', b']'),
            b'}' => (b'}', b'{'),
            b')' => (b')', b'('),
            b']' => (b']', b'['),
            _ => return None,
        };
        if matches!(byte, b'{' | b'(' | b'[') {
            Self::scan_forward(root, code.as_bytes(), pos, open, close)
        } else {
            Self::scan_backward(root, code.as_bytes(), pos, open, close)
        }
    }

    // -- Symbol at cursor -------------------------------------------------

    /// Determine what symbol sits under the cursor byte position.
    pub fn symbol_at(&mut self, code: &str, pos: usize) -> TsSymbol {
        let Some(tree) = self.parse(code) else {
            return TsSymbol::None;
        };
        Self::resolve_symbol(tree.root_node(), code, pos)
    }

    // -- Word at cursor ---------------------------------------------------

    /// Extract the identifier word at or near byte position `pos`.
    pub fn word_at(&mut self, code: &str, pos: usize) -> Option<String> {
        let tree = self.parse(code)?;
        let leaf = tree.root_node().named_descendant_for_byte_range(pos, pos)?;
        Self::node_text(code, leaf)
    }

    // -- Scope detection --------------------------------------------------

    /// Byte range (start, end) of the `function_definition` containing `pos`.
    pub fn enclosing_function(&mut self, code: &str, pos: usize) -> Option<(usize, usize)> {
        let tree = self.parse(code)?;
        let root = tree.root_node();
        let mut cursor = root.walk();
        if !cursor.goto_first_child() {
            return None;
        }
        loop {
            let node = cursor.node();
            if node.kind() == "function_definition" {
                let r = node.byte_range();
                if r.start <= pos && pos < r.end {
                    return Some((r.start, r.end));
                }
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
        None
    }

    // -- Reference finding ------------------------------------------------

    /// Find all byte ranges of `name` in `code` that are identifiers
    /// (not inside strings or comments).
    pub fn find_references(&mut self, code: &str, name: &str) -> Vec<usize> {
        let Some(tree) = self.parse(code) else {
            return vec![];
        };
        let root = tree.root_node();
        let bytes = code.as_bytes();
        let name_bytes = name.as_bytes();
        let mut refs = Vec::new();
        Self::collect_refs(root, bytes, name_bytes, root, &mut refs);
        refs
    }

    // -- Code-only search -------------------------------------------------

    /// Search for `pattern` in code only (skipping strings and comments), forward.
    /// Returns byte offset of first match, or None.
    pub fn code_search(&mut self, code: &str, pattern: &str, from: usize) -> Option<usize> {
        let tree = self.parse(code)?;
        let root = tree.root_node();
        let low = code.to_lowercase();
        let plow = pattern.to_lowercase();
        let mut pos = from;
        while pos <= code.len().saturating_sub(pattern.len()) {
            if let Some(offset) = low[pos..].find(&plow) {
                let abs = pos + offset;
                if !Self::in_string_or_comment(root, abs) {
                    return Some(abs);
                }
                pos = abs + 1;
            } else {
                break;
            }
        }
        None
    }

    /// Search backward for `pattern` in code only (skipping strings and comments).
    /// Returns byte offset of last match before `from`, or None.
    pub fn code_search_back(&mut self, code: &str, pattern: &str, from: usize) -> Option<usize> {
        let tree = self.parse(code)?;
        let root = tree.root_node();
        let low = code.to_lowercase();
        let plow = pattern.to_lowercase();
        let mut last: Option<usize> = None;
        let mut pos = 0;
        while pos <= from {
            if let Some(offset) = low[pos..].find(&plow) {
                let abs = pos + offset;
                if abs > from {
                    break;
                }
                if !Self::in_string_or_comment(root, abs) {
                    last = Some(abs);
                }
                pos = abs + 1;
            } else {
                break;
            }
        }
        last
    }

    /// Check if byte position is in code (not in string or comment).
    pub fn is_code(&mut self, code: &str, pos: usize) -> bool {
        // can't determine without a tree, so assume code
        self.parse(code)
            .is_none_or(|tree| !Self::in_string_or_comment(tree.root_node(), pos))
    }

    // -- Internal helpers ---------------------------------------------------

    fn in_string_or_comment(root: Node, pos: usize) -> bool {
        let Some(mut cur) = root.descendant_for_byte_range(pos, pos + 1) else {
            return false;
        };
        loop {
            match cur.kind() {
                "string_literal" | "system_lib_string" | "comment" | "block_comment"
                | "preproc_include" => return true,
                _ => {}
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => return false,
            }
        }
    }

    fn scan_forward(root: Node, bytes: &[u8], pos: usize, open: u8, close: u8) -> Option<usize> {
        let mut depth: i64 = 0;
        for (i, &b) in bytes.iter().enumerate().skip(pos) {
            if b == open && !Self::in_string_or_comment(root, i) {
                depth += 1;
            } else if b == close && !Self::in_string_or_comment(root, i) {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    fn scan_backward(root: Node, bytes: &[u8], pos: usize, open: u8, close: u8) -> Option<usize> {
        let mut depth: i64 = 0;
        for i in (0..=pos).rev() {
            let Some(&b) = bytes.get(i) else {
                continue;
            };
            if b == close && !Self::in_string_or_comment(root, i) {
                depth += 1;
            } else if b == open && !Self::in_string_or_comment(root, i) {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    fn node_text(code: &str, node: Node) -> Option<String> {
        code.get(node.byte_range()).map(ToString::to_string)
    }

    fn resolve_symbol(root: Node, code: &str, pos: usize) -> TsSymbol {
        let Some(innermost) = root.named_descendant_for_byte_range(pos, pos) else {
            return TsSymbol::None;
        };

        let mut in_fn = false;
        let mut cur = innermost;
        loop {
            if cur.kind() == "function_definition" {
                in_fn = true;
                break;
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => break,
            }
        }

        let Some(text) = Self::node_text(code, innermost) else {
            return TsSymbol::None;
        };

        match innermost.kind() {
            "identifier" => {
                if let Some(parent) = innermost.parent() {
                    if parent.kind() == "call_expression" {
                        return TsSymbol::Function { name: text, addr: 0 };
                    }
                }
                if in_fn {
                    TsSymbol::Local { name: text }
                } else {
                    TsSymbol::Global { name: text, addr: 0 }
                }
            }
            "type_identifier" => TsSymbol::Global { name: text, addr: 0 },
            "parameter_identifier" => TsSymbol::Param { name: text },
            _ => TsSymbol::None,
        }
    }

    fn collect_refs(node: Node, bytes: &[u8], name_bytes: &[u8], root: Node, refs: &mut Vec<usize>) {
        if node.kind() == "identifier" {
            let r = node.byte_range();
            if r.len() == name_bytes.len()
                && bytes.get(r.clone()) == Some(name_bytes)
                && !Self::in_string_or_comment(root, r.start)
            {
                refs.push(r.start);
            }
        }
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                Self::collect_refs(cursor.node(), bytes, name_bytes, root, refs);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsSymbol {
    Function { name: String, addr: u64 },
    Global { name: String, addr: u64 },
    Local { name: String },
    Param { name: String },
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    // "foo(bar(baz), qux)"
    //  0123456789012345678
    //          1111111111

    #[test]
    fn simple_braces() {
        let mut p = TsParser::new();
        let code = "void foo() { return; }";
        let open = code.find('{').unwrap();
        let close = p.find_match(code, open).unwrap();
        assert_eq!(code.as_bytes()[close], b'}');
    }

    #[test]
    fn nested_braces() {
        let mut p = TsParser::new();
        let code = "void foo() { if (x) { bar(); } }";
        let open = code.find('{').unwrap();
        let close = p.find_match(code, open).unwrap();
        assert_eq!(close, 31);
    }

    #[test]
    fn brace_in_string_not_matched() {
        let mut p = TsParser::new();
        let code = r#"char *s = "}"; void foo() { return; }"#;
        let open = code.find('{').unwrap();
        let close = p.find_match(code, open).unwrap();
        assert_eq!(close, code.rfind('}').unwrap());
    }

    #[test]
    fn parenthesis_nested() {
        let mut p = TsParser::new();
        let code = "foo(bar(baz), qux)";
        let open = code.find('(').unwrap();
        let close = p.find_match(code, open).unwrap();
        assert_eq!(close, 17);
    }

    #[test]
    fn parenthesis_inner() {
        let mut p = TsParser::new();
        let code = "foo(bar(baz), qux)";
        let open = code.find('(').unwrap() + 4;
        assert_eq!(code.as_bytes()[open], b'(');
        let close = p.find_match(code, open).unwrap();
        assert_eq!(close, 11);
    }

    #[test]
    fn bracket_in_comment_not_matched() {
        let mut p = TsParser::new();
        let code = "// }\nvoid foo() { return; }";
        let open = code.find('{').unwrap();
        let close = p.find_match(code, open).unwrap();
        assert_eq!(close, code.rfind('}').unwrap());
    }

    #[test]
    fn word_at_result() {
        let mut p = TsParser::new();
        let code = "int result = foo(bar);";
        assert_eq!(p.word_at(code, 4), Some("result".into()));
    }

    #[test]
    fn word_at_foo() {
        let mut p = TsParser::new();
        let code = "int result = foo(bar);";
        assert_eq!(p.word_at(code, 13), Some("foo".into()));
    }

    #[test]
    fn word_at_bar() {
        let mut p = TsParser::new();
        let code = "int result = foo(bar);";
        assert_eq!(p.word_at(code, 17), Some("bar".into()));
    }

    #[test]
    fn symbol_function_call() {
        let mut p = TsParser::new();
        let code = "int main() { foo(); return 0; }";
        let pos = code.find("foo").unwrap();
        let sym = p.symbol_at(code, pos);
        assert!(matches!(sym, TsSymbol::Function { name, .. } if name == "foo"));
    }

    #[test]
    fn symbol_local_var() {
        let mut p = TsParser::new();
        let code = "int main() { int x = 1; return x; }";
        let pos = code.find("return").unwrap() + 7;
        let sym = p.symbol_at(code, pos);
        assert!(matches!(sym, TsSymbol::Local { name } if name == "x"));
    }

    // -- Scope tests ------------------------------------------------------

    #[test]
    fn enclosing_function_simple() {
        let mut p = TsParser::new();
        let code = "void foo() { return; } void bar() { int x = 1; }";
        let body_pos = code.find("return").unwrap();
        let (start, end) = p.enclosing_function(code, body_pos).unwrap();
        assert_eq!(&code[start..end], "void foo() { return; }");
    }

    #[test]
    fn enclosing_function_second() {
        let mut p = TsParser::new();
        let code = "void foo() { return; } void bar() { int x = 1; }";
        let body_pos = code.find("int x").unwrap();
        let (start, end) = p.enclosing_function(code, body_pos).unwrap();
        assert_eq!(&code[start..end], "void bar() { int x = 1; }");
    }

    #[test]
    fn enclosing_function_outside() {
        let mut p = TsParser::new();
        let code = "int global = 0;";
        assert!(p.enclosing_function(code, 0).is_none());
    }

    // -- Reference tests --------------------------------------------------

    #[test]
    fn find_references_simple() {
        let mut p = TsParser::new();
        let code = "int foo(int x) { return x + foo(x - 1); }";
        let refs = p.find_references(code, "foo");
        assert_eq!(refs.len(), 2);
        assert_eq!(&code[refs[0]..refs[0] + 3], "foo");
        assert_eq!(&code[refs[1]..refs[1] + 3], "foo");
    }

    #[test]
    fn find_references_skips_strings() {
        let mut p = TsParser::new();
        let code = r#"int foo() { char *s = "foo"; return foo(); }"#;
        let refs = p.find_references(code, "foo");
        // Should find: function def, call inside body — but not the string
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn find_references_skips_comments() {
        let mut p = TsParser::new();
        let code = "// foo\nint foo() { return 0; }";
        let refs = p.find_references(code, "foo");
        // Should find: function name — but not the comment
        assert_eq!(refs.len(), 1);
    }

    // -- Code search tests ------------------------------------------------

    #[test]
    fn code_search_basic() {
        let mut p = TsParser::new();
        let code = "int x = 42; // 42\nchar *s = \"42\";";
        let pos = p.code_search(code, "42", 0).unwrap();
        assert_eq!(pos, 8); // in "int x = 42;"
    }

    #[test]
    fn code_search_skips_string() {
        let mut p = TsParser::new();
        let code = r#"char *s = "hello"; foo(hello);"#;
        let pos = p.code_search(code, "hello", 0).unwrap();
        assert_eq!(pos, code.find("foo(hello)").unwrap() + 4);
    }

    #[test]
    fn code_search_skips_comment() {
        let mut p = TsParser::new();
        let code = "// hello\nworld(hello);";
        let pos = p.code_search(code, "hello", 0).unwrap();
        assert_eq!(pos, code.find("world(hello)").unwrap() + 6);
    }

    #[test]
    fn code_search_back_finds_previous() {
        let mut p = TsParser::new();
        let code = "foo bar foo bar foo";
        // from after the third foo (pos 16), should find the second foo (pos 8)
        let pos = p.code_search_back(code, "foo", 15).unwrap();
        assert_eq!(pos, 8);
    }

    #[test]
    fn code_search_back_skips_strings() {
        let mut p = TsParser::new();
        let code = r#"foo "foo" foo"#;
        // from end, should find the third foo (pos 10), not the one in string (pos 4)
        let pos = p.code_search_back(code, "foo", 12).unwrap();
        assert_eq!(pos, 10);
    }

    #[test]
    fn code_search_back_from_middle() {
        let mut p = TsParser::new();
        let code = "aaa bbb aaa bbb aaa";
        // from pos 13 (second 'bbb'), should find second 'aaa' at pos 8
        let pos = p.code_search_back(code, "aaa", 13).unwrap();
        assert_eq!(pos, 8);
    }
}
