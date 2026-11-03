//! A small editable XML tree, enough for the documents of other VJ
//! applications: elements, attributes and text, no mixed content.
//!
//! Importers read it and exporters patch it, so that the parts of a document
//! Verso does not model are written back unchanged.

use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    /// Text content, for elements without children.
    pub text: String,
    pub children: Vec<Element>,
}

impl Element {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    /// Builder form of [`Element::set_attr`].
    pub fn with(mut self, name: &str, value: impl ToString) -> Self {
        self.set_attr(name, value);
        self
    }

    /// Builder form of pushing a child.
    pub fn child_of(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let doc = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
        Ok(Self::from_node(doc.root_element()))
    }

    fn from_node(node: roxmltree::Node) -> Self {
        let mut element = Element::new(node.tag_name().name());
        element.attrs = node
            .attributes()
            .map(|a| (a.name().to_owned(), a.value().to_owned()))
            .collect();
        for child in node.children() {
            if child.is_element() {
                element.children.push(Self::from_node(child));
            } else if let Some(text) = child.text() {
                element.text.push_str(text);
            }
        }
        if !element.children.is_empty() || element.text.trim().is_empty() {
            element.text.clear();
        }
        element
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_attr(&mut self, name: &str, value: impl ToString) {
        let value = value.to_string();
        match self.attrs.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value,
            None => self.attrs.push((name.to_owned(), value)),
        }
    }

    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut Element> {
        self.children.iter_mut().find(|c| c.name == name)
    }

    /// The child named `name`, appended first if missing.
    pub fn ensure_child(&mut self, name: &str) -> &mut Element {
        match self.children.iter().position(|c| c.name == name) {
            Some(i) => &mut self.children[i],
            None => {
                self.children.push(Element::new(name));
                self.children.last_mut().expect("just pushed")
            }
        }
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The document with an XML declaration, indented with tabs.
    pub fn to_document(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let indent = "\t".repeat(depth);
        let _ = write!(out, "{indent}<{}", self.name);
        for (name, value) in &self.attrs {
            let _ = write!(out, " {name}=\"{}\"", escape(value));
        }
        if self.children.is_empty() && self.text.is_empty() {
            out.push_str("/>\n");
        } else if self.children.is_empty() {
            let _ = writeln!(out, ">{}</{}>", escape(&self.text), self.name);
        } else {
            out.push_str(">\n");
            for child in &self.children {
                child.write(out, depth + 1);
            }
            let _ = writeln!(out, "{indent}</{}>", self.name);
        }
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let text = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<a n=\"x &amp; &quot;y&quot;\">\n\t<b/>\n\t<c>LLLL</c>\n</a>\n";
        let element = Element::parse(text).unwrap();
        assert_eq!(element.attr("n"), Some("x & \"y\""));
        assert_eq!(element.child("c").unwrap().text, "LLLL");
        assert_eq!(element.to_document(), text);
    }
}
