//! Element references.

use std::ops::Deref;

use ego_tree::iter::{Edge, Traverse};
use ego_tree::NodeRef;
use fast_html5ever::serialize::{serialize, SerializeOpts, TraversalScope};

use crate::node::Element;
use crate::node::Node;
use crate::selector::Selector;

/// Wrapper around a reference to an element node.
///
/// This wrapper implements the `Element` trait from the `selectors` crate, which allows it to be
/// matched against CSS selectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementRef<'a> {
    node: NodeRef<'a, Node>,
    /// The language of the element. Not used atm.
    pub lang: &'a str,
}

impl<'a> ElementRef<'a> {
    fn new(node: NodeRef<'a, Node>) -> Self {
        ElementRef { node, lang: "" }
    }

    /// Wraps a `NodeRef` only if it references a `Node::Element`.
    pub fn wrap(node: NodeRef<'a, Node>) -> Option<Self> {
        if node.value().is_element() {
            Some(ElementRef::new(node))
        } else {
            None
        }
    }

    /// Returns the `Element` referenced by `self`.
    pub fn value(&self) -> &'a Element {
        self.node.value().as_element().unwrap()
    }

    /// Returns an iterator over descendent elements matching a selector.
    pub fn select<'b>(&self, selector: &'b Selector) -> Select<'a, 'b> {
        let mut inner = self.traverse();
        inner.next(); // Skip Edge::Open(self).

        Select {
            scope: *self,
            inner,
            selector,
        }
    }

    fn serialize(&self, traversal_scope: TraversalScope) -> String {
        let opts = SerializeOpts {
            scripting_enabled: false, // It's not clear what this does.
            traversal_scope,
            create_missing_parent: false,
        };
        let mut buf = Vec::new();
        let _ = serialize(&mut buf, self, opts);
        // we need to get the initial encoding of the html lang if used.
        auto_encoder::auto_encode_bytes(&buf)
    }

    /// Returns the HTML of this element.
    pub fn html(&self) -> String {
        self.serialize(TraversalScope::IncludeNode)
    }

    /// Returns the inner HTML of this element.
    pub fn inner_html(&self) -> String {
        self.serialize(TraversalScope::ChildrenOnly(None))
    }

    /// Returns the value of an attribute.
    pub fn attr(&self, attr: &str) -> Option<&str> {
        self.value().attr(attr)
    }

    /// Returns an iterator over descendent text nodes.
    pub fn text(&self) -> Text<'a> {
        Text {
            inner: self.traverse(),
        }
    }
}

impl<'a> Deref for ElementRef<'a> {
    type Target = NodeRef<'a, Node>;
    fn deref(&self) -> &NodeRef<'a, Node> {
        &self.node
    }
}

/// Iterator over descendent elements matching a selector.
#[derive(Debug, Clone)]
pub struct Select<'a, 'b> {
    scope: ElementRef<'a>,
    inner: Traverse<'a, Node>,
    selector: &'b Selector,
}

impl<'a, 'b> Iterator for Select<'a, 'b> {
    type Item = ElementRef<'a>;

    fn next(&mut self) -> Option<ElementRef<'a>> {
        for edge in &mut self.inner {
            if let Edge::Open(node) = edge {
                if let Some(element) = ElementRef::wrap(node) {
                    if self.selector.matches_with_scope(&element, Some(self.scope)) {
                        return Some(element);
                    }
                }
            }
        }
        None
    }
}

/// Iterator over descendent text nodes.
#[derive(Debug, Clone)]
pub struct Text<'a> {
    inner: Traverse<'a, Node>,
}

impl<'a> Iterator for Text<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        for edge in &mut self.inner {
            if let Edge::Open(ref node) = edge {
                // check if the element is not a script or link.
                let processable = match node.parent() {
                    Some(e) => {
                        match e.value().as_element() {
                            Some(n) => {
                                let name = n.name();
                                // prevent all script and style elements
                                !(name == "script" || name == "style")
                            }
                            _ => true,
                        }
                    }
                    _ => true,
                };

                if !processable {
                    continue;
                }

                if let Node::Text(text) = node.value() {
                    return Some(&**text);
                }
            }
        }

        None
    }
}

mod element;
mod serializable;

#[cfg(test)]
mod tests {
    use crate::html::Html;
    use crate::selector::Selector;

    #[test]
    fn test_scope() {
        let html = r"
            <div>
                <b>1</b>
                <span>
                    <span><b>2</b></span>
                    <b>3</b>
                </span>
            </div>
        ";
        let fragment = Html::parse_fragment(html);
        let sel1 = Selector::parse("div > span").unwrap();
        let sel2 = Selector::parse(":scope > b").unwrap();

        let element1 = fragment.select(&sel1).next().unwrap();
        let element2 = element1.select(&sel2).next().unwrap();
        assert_eq!(element2.inner_html(), "3");
    }

    #[test]
    fn test_text() {
        let fragment = Html::parse_fragment("<h1>Hello, <i>world!</i></h1><script>window.var = true</script><style>.main { background: white };</style>");
        let selector = Selector::parse("h1").unwrap();

        let h1 = fragment.select(&selector).next().unwrap();
        let text = h1.text().collect::<Vec<_>>();

        assert_eq!(vec!["Hello, ", "world!"], text);
    }
}
