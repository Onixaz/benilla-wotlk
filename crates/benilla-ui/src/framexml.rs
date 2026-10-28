//! The FrameXML document layer: a `.xml` file's text as an owned, order-preserving tree, with
//! `inherits` template expansion and `$parent` name substitution; no widgets and no Lua. It
//! follows the reference's loader (`0x6edc00`–`0x6f3000`, the file loader `0x6ede10`), whose XML
//! tokenizer is an embedded expat and ours `roxmltree`.

use std::collections::{HashMap, HashSet};
use std::fmt;

/// An owned XML element, attributes in document order. Lookups fold case, as the reference's
/// `GetAttribute 0x6f2cf0` and element-name compares (`0x64a4c0`) do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub tag: String,
    attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// The node's own direct text (the reference node's body text at `+0xc`): a handler or
    /// `<Script>` body.
    pub body: String,
}

impl Element {
    /// Case-insensitive attribute lookup.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// True only for a case-insensitive `"true"` (`0x6f1b30`); there is no `"false"` branch.
    pub fn attr_bool(&self, name: &str) -> bool {
        self.attr(name)
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    }

    /// A flag read by `StringToBOOL` (`0x815400`), `None` when absent or empty, as the reference
    /// skips the setter then (`<EditBox autoFocus="false">`, `password="1"`).
    pub fn attr_bool_opt(&self, name: &str) -> Option<bool> {
        self.attr(name)
            .filter(|v| !v.is_empty())
            .map(string_to_bool)
    }

    pub fn name(&self) -> Option<&str> {
        self.attr("name")
    }

    pub fn attrs(&self) -> &[(String, String)] {
        &self.attrs
    }
}

/// `StringToBOOL` (`0x815400`): the first character decides (`0`, `F`, `N` false; a digit, `T`, `Y`
/// true), else `off`/`disabled` and `on`/`enabled`, case-folded; anything else is false.
pub fn string_to_bool(v: &str) -> bool {
    match v.as_bytes().first() {
        Some(b'0' | b'F' | b'N' | b'f' | b'n') => false,
        Some(b'1'..=b'9' | b'T' | b'Y' | b't' | b'y') => true,
        _ => v.eq_ignore_ascii_case("on") || v.eq_ignore_ascii_case("enabled"),
    }
}

/// A `<Script file=…>` or an inline `<Script>` body; both run in document order with the rest of
/// the top level (`0x704bc0`/`0x704cd0`, from `0x6ede10`'s walk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptRef {
    File(String),
    Inline {
        body: String,
        /// The 1-based line of the file that `body` starts on, so the loader can pad the chunk and
        /// Lua's error lines are the file's.
        line: u32,
    },
}

/// One top-level item, kept in document order: the reference runs `<Include>` and `<Script>`
/// interleaved with frame definitions (`0x6ede10`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopLevel {
    /// `<Include file=…>`: load another file at this point (`0x8710c0`).
    Include(String),
    Script(ScriptRef),
    /// `<Font …>`, a named font definition (`0x87106c`).
    Font(Element),
    /// An element with `virtual="true"`: a template keyed by `name`, not instantiated
    /// (`0x6ee500`).
    Template(Element),
    /// An element without `virtual="true"`: instantiated in place.
    Instance(Element),
}

/// One parsed document: its top-level items in order, and the issues it loaded past.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedDocument {
    pub items: Vec<TopLevel>,
    /// An unnamed virtual, an `<Include>` or `<Font>` missing its attribute, a root that is not
    /// `<Ui>`: logged, and the file goes on.
    pub warnings: Vec<String>,
}

impl ParsedDocument {
    /// The named templates, for [`expand`]; an unnamed one (the reference's "Unnamed virtual
    /// node" error, `0x6ee500`) is absent.
    pub fn templates(&self) -> HashMap<&str, &Element> {
        self.items
            .iter()
            .filter_map(|item| match item {
                TopLevel::Template(el) => el.name().map(|name| (name, el)),
                _ => None,
            })
            .collect()
    }
}

/// Malformed XML; anything softer is a [`ParsedDocument::warnings`] entry.
#[derive(Debug)]
pub enum Error {
    Xml(roxmltree::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Xml(e) => write!(f, "malformed FrameXML document: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Xml(e) => Some(e),
        }
    }
}

/// Parse FrameXML text. As in `0x6ede10`, the root's children are walked in order: `<Include>`
/// and `<Script>` keep their place, `<Font>` is a font, and any other element, known tag or not,
/// is a template when `virtual="true"` and an instance otherwise; widget types are checked at
/// instantiation. A root that is not `<Ui>` only warns: the reference's walk is not known to check
/// the root's tag.
pub fn parse(text: &str) -> Result<ParsedDocument, Error> {
    // The reference's expat is created by `XML_ParserCreate 0x7e6690`, not the NS variant, so a
    // prefix is opaque and an undeclared one (`xsi:` with no `xmlns:xsi`) is no error, where
    // `roxmltree` rejects the whole document. Bind the prefix the error names and re-read; the
    // bytes are otherwise untouched, and `element_from_node` drops prefixed attributes.
    let repaired;
    let doc = match roxmltree::Document::parse(text) {
        Ok(doc) => doc,
        Err(roxmltree::Error::UnknownNamespace(..)) => {
            repaired = bind_undeclared_prefixes(text);
            roxmltree::Document::parse(&repaired).map_err(Error::Xml)?
        }
        Err(e) => return Err(Error::Xml(e)),
    };
    let root = doc.root_element();

    let mut warnings = Vec::new();
    if !root.tag_name().name().eq_ignore_ascii_case("Ui") {
        warnings.push(format!(
            "root element is <{}>, not <Ui>; tolerated (the reference's file loader walks \
             the root's children regardless of the root's own tag)",
            root.tag_name().name()
        ));
    }

    let mut items = Vec::new();
    for child in root.children().filter(|n| n.is_element()) {
        let tag = child.tag_name().name();
        if tag.eq_ignore_ascii_case("Include") {
            match attr_ci(child, "file") {
                Some(file) => items.push(TopLevel::Include(file)),
                None => warnings.push("<Include> without a file attribute; skipped".to_string()),
            }
        } else if tag.eq_ignore_ascii_case("Script") {
            match attr_ci(child, "file") {
                Some(file) => items.push(TopLevel::Script(ScriptRef::File(file))),
                None => items.push(TopLevel::Script(ScriptRef::Inline {
                    body: direct_text(child),
                    line: inline_start_line(&doc, child),
                })),
            }
        } else if tag.eq_ignore_ascii_case("Font") {
            let el = element_from_node(child);
            if el.name().is_none() {
                warnings.push("<Font> without a name attribute".to_string());
            }
            items.push(TopLevel::Font(el));
        } else {
            let el = element_from_node(child);
            if el.attr_bool("virtual") {
                if el.name().is_none() {
                    warnings.push(format!(
                        "virtual <{}> without a name — a load error in the real client \
                         (\"Unnamed virtual node\"); kept but unregistered, \
                         so nothing can inherit it",
                        el.tag
                    ));
                }
                items.push(TopLevel::Template(el));
            } else {
                items.push(TopLevel::Instance(el));
            }
        }
    }

    Ok(ParsedDocument { items, warnings })
}

/// Declare each prefix the document uses but never binds, one per round on the root's start tag
/// (in scope everywhere, the root's own name included), taking the prefix each
/// `UnknownNamespace` error names, never a text scan, which would match inside comments and
/// values. Any other error, or [`MAX_BOUND_PREFIXES`] rounds, returns the text as it stands.
pub(crate) fn bind_undeclared_prefixes(text: &str) -> String {
    let mut out = text.to_string();
    for _ in 0..MAX_BOUND_PREFIXES {
        let Err(roxmltree::Error::UnknownNamespace(prefix, _)) = roxmltree::Document::parse(&out)
        else {
            return out;
        };
        let Some(at) = root_attr_insertion_point(&out) else {
            return out;
        };
        // An inert URI, distinct per prefix so two stray prefixes stay two namespaces.
        out.insert_str(
            at,
            &format!(" xmlns:{prefix}=\"urn:benilla:undeclared:{prefix}\""),
        );
    }
    out
}

/// The most undeclared prefixes [`bind_undeclared_prefixes`] binds before giving up.
const MAX_BOUND_PREFIXES: usize = 8;

/// The byte offset just before the `>` or `/>` closing the root's start tag, found by hand since
/// the document does not parse yet: the prolog is skipped and quoted values are stepped over, so
/// a `>` inside one (`hyperlinkFormat="|H%s|h[%s]|h"`) is not the tag's end.
fn root_attr_insertion_point(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        if rest.starts_with("<?") {
            i += rest.find("?>").map(|n| n + 2)?;
        } else if rest.starts_with("<!--") {
            i += rest.find("-->").map(|n| n + 3)?;
        } else if rest.starts_with("<!") {
            i += rest.find('>').map(|n| n + 1)?;
        } else {
            // The root's start tag.
            let mut j = i + 1;
            while j < b.len() {
                match b[j] {
                    q @ (b'\'' | b'"') => {
                        j += 1;
                        while j < b.len() && b[j] != q {
                            j += 1;
                        }
                    }
                    b'>' => {
                        // Insert before a `/>`'s slash, not between the slash and the `>`.
                        let at = if j > 0 && b[j - 1] == b'/' { j - 1 } else { j };
                        return Some(at);
                    }
                    _ => {}
                }
                j += 1;
            }
            return None;
        }
    }
    None
}

fn attr_ci(node: roxmltree::Node, name: &str) -> Option<String> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value().to_string())
}

/// The 1-based line of the first text byte inside a `<Script>`: the first text child's, since
/// `<![CDATA[` can sit between the tag and the body, else the element's own.
fn inline_start_line(doc: &roxmltree::Document, node: roxmltree::Node) -> u32 {
    let at = node
        .children()
        .find(|n| n.is_text())
        .unwrap_or(node)
        .range()
        .start;
    doc.text_pos_at(at).row
}

fn direct_text(node: roxmltree::Node) -> String {
    node.children()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

fn element_from_node(node: roxmltree::Node) -> Element {
    Element {
        tag: node.tag_name().name().to_string(),
        attrs: node
            .attributes()
            // `GetAttribute 0x6f2cf0` matches the whole stored name and no FrameXML key has a
            // colon, so a prefixed attribute is unreachable; dropping it also stops `roxmltree`'s
            // local names from letting `xsi:name` answer for `name`.
            .filter(|a| a.namespace().is_none())
            .map(|a| (a.name().to_string(), a.value().to_string()))
            .collect(),
        children: node
            .children()
            .filter(|n| n.is_element())
            .map(element_from_node)
            .collect(),
        body: direct_text(node),
    }
}

/// Resolve `inherits` into a materialized [`Element`]: the named template, itself expanded first,
/// then the element on top. Children are the template's then the element's; the element's
/// attributes override or extend the template's (case-insensitively), all but `name`, `id` and
/// `virtual`, which are the instance's own: `FrameXML_CreateFrame` (`0x812fa0`) runs `PreLoadXML`
/// (`0x490360`, `0x4814f0` for a region), which reads `name` and `id`, on the instance node alone,
/// then `LoadXML` walks the templates. A cycle is skipped with a warning.
pub fn expand(
    element: &Element,
    templates: &HashMap<&str, &Element>,
    warnings: &mut Vec<String>,
) -> Element {
    expand_known(element, templates, &HashSet::new(), warnings)
}

/// [`expand`], skipping without a warning an `inherits` that names a font object: a
/// `<FontString inherits="GameFontNormalSmall">` takes a font, applied later, not a template.
pub fn expand_known(
    element: &Element,
    templates: &HashMap<&str, &Element>,
    fonts: &HashSet<&str>,
    warnings: &mut Vec<String>,
) -> Element {
    let mut active = HashSet::new();
    expand_inner(element, templates, fonts, warnings, &mut active)
}

/// The template names an `inherits=` value names, in application order: the value split on
/// commas and each name trimmed, the empty ones skipped. The shipped FrameXML names lists both
/// ways, `"ChatFrameTemplate,FloatingBorderedFrame"` and `"SecureActionButtonTemplate,
/// ActionButtonTemplate"`. A name is matched case-insensitively, ASCII only.
pub fn inherit_names(inherits: &str) -> Vec<&str> {
    inherits
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn expand_inner(
    element: &Element,
    templates: &HashMap<&str, &Element>,
    fonts: &HashSet<&str>,
    warnings: &mut Vec<String>,
    active: &mut HashSet<String>,
) -> Element {
    let Some(inherits) = element.attr("inherits") else {
        return element.clone();
    };

    let mut base: Option<Element> = None;
    for name in inherit_names(inherits) {
        if !active.insert(name.to_string()) {
            warnings.push(format!(
                "inheritance cycle detected at template '{name}'; skipping this reference"
            ));
            continue;
        }
        let hit = templates.get(name).copied().or_else(|| {
            templates
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| *v)
        });
        let Some(template) = hit else {
            // A font object is not an unknown template; it is applied after this.
            if !fonts.contains(name) {
                warnings.push(format!(
                    "unknown template '{name}' referenced by inherits; skipping"
                ));
            }
            active.remove(name);
            continue;
        };
        let expanded_template = expand_inner(template, templates, fonts, warnings, active);
        active.remove(name);
        base = Some(match base {
            None => expanded_template,
            Some(acc) => merge(&acc, &expanded_template),
        });
    }

    match base {
        Some(mut base) => {
            base.attrs.retain(|(k, _)| {
                !["name", "id", "virtual"]
                    .iter()
                    .any(|own| k.eq_ignore_ascii_case(own))
            });
            merge(&base, element)
        }
        None => element.clone(),
    }
}

/// [`expand`]'s merge; `over`'s tag wins, and its body when it has one.
fn merge(base: &Element, over: &Element) -> Element {
    let mut attrs = base.attrs.clone();
    for (k, v) in &over.attrs {
        if let Some(slot) = attrs.iter_mut().find(|(ek, _)| ek.eq_ignore_ascii_case(k)) {
            slot.1 = v.clone();
        } else {
            attrs.push((k.clone(), v.clone()));
        }
    }

    let mut children = base.children.clone();
    children.extend(over.children.iter().cloned());

    Element {
        tag: over.tag.clone(),
        attrs,
        children,
        body: if over.body.is_empty() {
            base.body.clone()
        } else {
            over.body.clone()
        },
    }
}

/// The bare node a runtime `CreateFrame(kind, name, parent, inherits)` expands through
/// [`expand`], so Lua and XML resolve `inherits` alike. The result keeps `tag`, the kind
/// `CreateFrame` was given: a template's `<Button>` cannot retype a `"Frame"`.
pub fn inherits_node(tag: &str, inherits: &str) -> Element {
    Element {
        tag: tag.to_string(),
        attrs: vec![("inherits".to_string(), inherits.to_string())],
        children: Vec::new(),
        body: String::new(),
    }
}

/// What `$parent` expands to with no named ancestor: the literal `"Top"` (`0x8788ac`).
pub const DEFAULT_PARENT_NAME: &str = "Top";

/// `$parent` substitution (`0x76c5b0`): a leading `$parent`, case-insensitive and the only such
/// token, becomes `parent_name`, the rest appended verbatim. `parent_name` is the already-resolved
/// name of the nearest named ancestor (the `+0x9c` walk), else [`DEFAULT_PARENT_NAME`], so nested
/// `$parent` names compose. The reference calls it from two sites only, in `SetName` (`0x76c691`)
/// and in the layout resolver `0x76c700` (`0x76c71c`): a name and an anchor's `relativeTo` (XML,
/// `SetPoint` or `SetAllPoints`) expand, and `parent=` never does.
pub fn resolve_name(raw: &str, parent_name: &str) -> String {
    const TOKEN: &str = "$parent";
    match raw.get(..TOKEN.len()) {
        Some(prefix) if prefix.eq_ignore_ascii_case(TOKEN) => {
            format!("{parent_name}{}", &raw[TOKEN.len()..])
        }
        _ => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_undeclared_namespace_prefix_does_not_cost_the_document() {
        let doc = parse(
            r#"<Ui xsi:schemaLocation="http://www.blizzard.com/wow/ui/">
                <Frame name="Survived"/>
            </Ui>"#,
        )
        .expect("the reference parses this, so we must");
        assert_eq!(doc.items.len(), 1);
        let TopLevel::Instance(el) = &doc.items[0] else {
            panic!("{:?}", doc.items[0])
        };
        assert_eq!(el.name(), Some("Survived"));
    }

    /// One stray prefix is on an element, not an attribute.
    #[test]
    fn several_undeclared_prefixes_are_all_bound() {
        let doc = parse(
            r#"<Ui xsi:schemaLocation="x" foo:bar="y">
                <Frame name="A"/>
                <bar:Frame name="B"/>
            </Ui>"#,
        )
        .expect("every stray prefix is bound, not just the first");
        assert_eq!(doc.items.len(), 2);
    }

    /// Both spellings, since stock FrameXML declares `xmlns:xsi` on every `<Ui>`.
    #[test]
    fn a_prefixed_attribute_never_answers_an_unprefixed_lookup() {
        for text in [
            r#"<Ui><Frame xsi:name="Ghost" text="real"/></Ui>"#,
            r#"<Ui xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
                 <Frame xsi:name="Ghost" text="real"/>
               </Ui>"#,
        ] {
            let doc = parse(text).unwrap();
            let TopLevel::Instance(el) = &doc.items[0] else {
                panic!()
            };
            assert_eq!(el.name(), None, "a prefixed name= is unreachable: {text}");
            assert_eq!(el.attr("text"), Some("real"), "{text}");
        }
    }

    #[test]
    fn a_malformed_document_still_fails_with_its_own_error() {
        assert!(parse(r#"<Ui xsi:a="b"><Frame></Ui>"#).is_err());
    }

    #[test]
    fn the_root_tag_is_found_past_a_prolog_and_a_quoted_angle_bracket() {
        let doc = parse(
            "<?xml version=\"1.0\"?>\n<!-- a > in a comment -->\n\
             <Ui xsi:a=\"b\" note=\"a &gt; b\"><Frame name=\"Ok\"/></Ui>",
        )
        .unwrap();
        assert_eq!(doc.items.len(), 1);
        // A self-closing root: the binding goes before the slash.
        assert!(parse(r#"<Ui xsi:a="b"/>"#).is_ok());
    }

    #[test]
    fn top_level_order_preserves_script_and_element_interleave() {
        let doc = parse(
            r#"<Ui>
                <Script file="A.lua"/>
                <Frame name="First"/>
                <Script>print("inline")</Script>
                <Include file="Other.xml"/>
                <Frame name="Second"/>
            </Ui>"#,
        )
        .unwrap();

        let kinds: Vec<&str> = doc
            .items
            .iter()
            .map(|item| match item {
                TopLevel::Include(_) => "include",
                TopLevel::Script(ScriptRef::File(_)) => "script-file",
                TopLevel::Script(ScriptRef::Inline { .. }) => "script-inline",
                TopLevel::Font(_) => "font",
                TopLevel::Template(_) => "template",
                TopLevel::Instance(_) => "instance",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "script-file",
                "instance",
                "script-inline",
                "include",
                "instance"
            ],
            "document order must be preserved, not grouped by kind"
        );

        match &doc.items[0] {
            TopLevel::Script(ScriptRef::File(f)) => assert_eq!(f, "A.lua"),
            other => panic!("expected file script, got {other:?}"),
        }
        assert!(doc.warnings.is_empty());
    }

    #[test]
    fn inline_vs_file_script() {
        let doc = parse(
            r#"<Ui>
                <Script file="Foo.lua"/>
                <Script>local x = 1
print(x)</Script>
            </Ui>"#,
        )
        .unwrap();
        assert_eq!(
            doc.items[0],
            TopLevel::Script(ScriptRef::File("Foo.lua".to_string()))
        );
        match &doc.items[1] {
            TopLevel::Script(ScriptRef::Inline { body, line }) => {
                assert!(body.contains("local x = 1"));
                assert!(body.contains("print(x)"));
                // Line 3 of the literal above: `<Ui>`, `<Script file=…>`, then this one.
                assert_eq!(*line, 3, "the inline body's own line in the file");
            }
            other => panic!("expected inline script, got {other:?}"),
        }
    }

    #[test]
    fn virtual_registers_as_template_non_virtual_as_instance() {
        let doc = parse(
            r#"<Ui>
                <Frame name="MyTemplate" virtual="true"/>
                <Frame name="MyInstance"/>
                <Frame VIRTUAL="TRUE" name="CasedTemplate"/>
            </Ui>"#,
        )
        .unwrap();
        assert!(matches!(doc.items[0], TopLevel::Template(_)));
        assert!(matches!(doc.items[1], TopLevel::Instance(_)));
        assert!(
            matches!(doc.items[2], TopLevel::Template(_)),
            "virtual attribute name/value compare is case-insensitive"
        );
        let templates = doc.templates();
        assert!(templates.contains_key("MyTemplate"));
        assert!(templates.contains_key("CasedTemplate"));
        assert_eq!(templates.len(), 2);
    }

    #[test]
    fn unnamed_virtual_warns_but_does_not_fail() {
        let doc = parse(r#"<Ui><Frame virtual="true"/></Ui>"#).unwrap();
        assert!(matches!(doc.items[0], TopLevel::Template(_)));
        assert_eq!(doc.warnings.len(), 1);
        assert!(
            doc.warnings[0].contains("Unnamed virtual node")
                || doc.warnings[0].contains("without a name")
        );
        assert!(
            doc.templates().is_empty(),
            "an unnamed template cannot be inherited"
        );
    }

    #[test]
    fn unknown_top_level_tag_is_tolerated_as_generic_element() {
        let doc = parse(r#"<Ui><TotallyMadeUpTag name="Whatever" foo="bar"/></Ui>"#).unwrap();
        match &doc.items[0] {
            TopLevel::Instance(el) => {
                assert_eq!(el.tag, "TotallyMadeUpTag");
                assert_eq!(el.attr("foo"), Some("bar"));
            }
            other => panic!("expected a generic instance, got {other:?}"),
        }
        assert!(doc.warnings.is_empty());
    }

    #[test]
    fn inheritance_chain_recurses() {
        let doc = parse(
            r#"<Ui>
                <Frame name="Root" virtual="true" frameStrata="LOW">
                    <Layers><Layer level="ARTWORK"><Texture name="RootTex"/></Layer></Layers>
                </Frame>
                <Frame name="Mid" virtual="true" inherits="Root" frameStrata="MEDIUM">
                    <Layers><Layer level="ARTWORK"><Texture name="MidTex"/></Layer></Layers>
                </Frame>
                <Frame name="Leaf" inherits="Mid"/>
            </Ui>"#,
        )
        .unwrap();
        let templates = doc.templates();
        let TopLevel::Instance(leaf) = &doc.items[2] else {
            panic!("expected instance")
        };
        let mut warnings = Vec::new();
        let expanded = expand(leaf, &templates, &mut warnings);
        assert!(warnings.is_empty());
        // Mid's frameStrata overrides Root's, and Leaf sets none.
        assert_eq!(expanded.attr("frameStrata"), Some("MEDIUM"));
        assert_eq!(
            expanded.children.len(),
            2,
            "Root's then Mid's <Layers>, in order"
        );
    }

    /// `PreLoadXML` (`0x490360`) reads `name` and `id` off the instance node alone, so an unnamed
    /// child built from a named template stays unnamed and its `$parent` children take the
    /// nearest named ancestor, not the template's name.
    #[test]
    fn an_instance_takes_neither_name_id_nor_virtual_from_its_template() {
        let doc = parse(
            r#"<Ui>
                <CheckButton name="CheckTemplate" virtual="true" id="7" frameStrata="LOW"/>
                <CheckButton inherits="CheckTemplate"/>
                <CheckButton name="Named" id="2" inherits="CheckTemplate"/>
            </Ui>"#,
        )
        .unwrap();
        let templates = doc.templates();
        let mut warnings = Vec::new();
        let TopLevel::Instance(bare) = &doc.items[1] else {
            panic!("expected instance")
        };
        let bare = expand(bare, &templates, &mut warnings);
        assert_eq!(bare.attr("name"), None, "an unnamed instance stays unnamed");
        assert_eq!(bare.attr("id"), None);
        assert_eq!(bare.attr("virtual"), None, "nor is it a template");
        assert_eq!(
            bare.attr("frameStrata"),
            Some("LOW"),
            "other attributes inherit"
        );
        let TopLevel::Instance(named) = &doc.items[2] else {
            panic!("expected instance")
        };
        let named = expand(named, &templates, &mut warnings);
        assert_eq!(
            (named.attr("name"), named.attr("id")),
            (Some("Named"), Some("2"))
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn inheritance_cycle_warns_and_terminates() {
        let doc = parse(
            r#"<Ui>
                <Frame name="A" virtual="true" inherits="B"/>
                <Frame name="B" virtual="true" inherits="A"/>
                <Frame name="Inst" inherits="A"/>
            </Ui>"#,
        )
        .unwrap();
        let templates = doc.templates();
        let TopLevel::Instance(inst) = &doc.items[2] else {
            panic!("expected instance")
        };
        let mut warnings = Vec::new();
        let _expanded = expand(inst, &templates, &mut warnings);
        assert!(
            warnings.iter().any(|w| w.contains("cycle")),
            "expected a cycle warning, got {warnings:?}"
        );
    }

    #[test]
    fn parent_name_substitution_basic_and_case_insensitive() {
        assert_eq!(
            resolve_name("$parentHealthBar", "PlayerFrame"),
            "PlayerFrameHealthBar"
        );
        assert_eq!(
            resolve_name("$PARENThealthbar", "PlayerFrame"),
            "PlayerFramehealthbar"
        );
        assert_eq!(resolve_name("$Parent", "PlayerFrame"), "PlayerFrame");
        assert_eq!(
            resolve_name("NotAParentName", "PlayerFrame"),
            "NotAParentName"
        );
        // No named ancestor: the caller passes the literal "Top".
        assert_eq!(resolve_name("$parentFoo", DEFAULT_PARENT_NAME), "TopFoo");
    }

    #[test]
    fn parent_name_substitution_composes_through_nested_children() {
        // The grandchild resolves against the child's resolved name, not its raw "$parent…" text.
        let parent_name = "PlayerFrame";
        let child_name = resolve_name("$parentHealthBar", parent_name);
        assert_eq!(child_name, "PlayerFrameHealthBar");
        let grandchild_name = resolve_name("$parentText", &child_name);
        assert_eq!(grandchild_name, "PlayerFrameHealthBarText");
    }
}
