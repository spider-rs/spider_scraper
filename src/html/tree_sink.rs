use super::Html;
use crate::node::{Doctype, Element, Node, ProcessingInstruction, Text};
use ego_tree::{NodeId, Tree};
use html5ever::interface::ElemName;
use html5ever::tendril::StrTendril;
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::Attribute;
use html5ever::QualName;
use html5ever::{LocalName, Namespace};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};

/// Owned `ElemName` implementation.
///
/// `html5ever` 0.39's `TreeSink::ElemName<'a>` requires the returned name
/// to outlive the borrow of `self`. We can't return a reference into the
/// `RefCell<Tree>` because the `Ref` guard would die at function return.
/// Cloning the cheap, `Arc`-backed `Namespace` and `LocalName` atoms
/// sidesteps the lifetime/aliasing problem with no `unsafe`.
#[derive(Debug)]
pub(crate) struct OwnedElemName {
    ns: Namespace,
    local: LocalName,
}

impl ElemName for OwnedElemName {
    fn ns(&self) -> &Namespace {
        &self.ns
    }

    fn local_name(&self) -> &LocalName {
        &self.local
    }
}

impl OwnedElemName {
    /// Sentinel used when the parser asks for `elem_name` of a node that
    /// somehow isn't an element. Should never happen under the parser's
    /// invariants, but we'd rather return a placeholder than panic.
    fn sentinel() -> Self {
        OwnedElemName {
            ns: Namespace::default(),
            local: LocalName::default(),
        }
    }
}

/// Internal builder used during parsing.
///
/// `html5ever` 0.39's `TreeSink` callbacks take `&self`, so the builder
/// uses interior mutability via `RefCell<Tree<Node>>`. After parsing
/// finishes, we collapse it into a plain `Html` so the public API stays
/// borrow-friendly (no `RefCell` on `Html` itself, no API churn for
/// `select` / `root_element` / `ElementRef`).
///
/// Every `TreeSink` callback that touches the tree is defensive: an
/// invalid `NodeId` from the parser is silently ignored rather than
/// triggering an `unwrap()` panic. Under the parser's invariants this
/// never fires; the defensiveness exists to keep us crash-free even if
/// upstream html5ever ever produces an unexpected handle.
#[derive(Debug)]
pub(crate) struct HtmlBuilder {
    quirks_mode: Cell<QuirksMode>,
    tree: RefCell<Tree<Node>>,
}

impl HtmlBuilder {
    pub(crate) fn new_document() -> Self {
        HtmlBuilder {
            quirks_mode: Cell::new(QuirksMode::NoQuirks),
            tree: RefCell::new(Tree::new(Node::Document)),
        }
    }

    pub(crate) fn new_fragment() -> Self {
        HtmlBuilder {
            quirks_mode: Cell::new(QuirksMode::NoQuirks),
            tree: RefCell::new(Tree::new(Node::Fragment)),
        }
    }
}

impl TreeSink for HtmlBuilder {
    type Output = Html;
    type Handle = NodeId;
    type ElemName<'a>
        = OwnedElemName
    where
        Self: 'a;

    fn finish(self) -> Html {
        Html {
            quirks_mode: self.quirks_mode.into_inner(),
            tree: self.tree.into_inner(),
            lang: String::new(),
        }
    }

    fn parse_error(&self, _: Cow<'static, str>) {}

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.quirks_mode.set(mode);
    }

    fn get_document(&self) -> Self::Handle {
        self.tree.borrow().root().id()
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        x == y
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> OwnedElemName {
        let tree = self.tree.borrow();
        let Some(node) = tree.get(*target) else {
            return OwnedElemName::sentinel();
        };
        let Some(elem) = node.value().as_element() else {
            return OwnedElemName::sentinel();
        };
        OwnedElemName {
            ns: elem.name.ns.clone(),
            local: elem.name.local.clone(),
        }
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> Self::Handle {
        let mut tree = self.tree.borrow_mut();
        let mut node = tree.orphan(Node::Element(Element::new(name.clone(), attrs)));
        if name.expanded() == expanded_name!(html "template") {
            node.append(Node::Fragment);
        }
        node.id()
    }

    fn create_comment(&self, _text: StrTendril) -> Self::Handle {
        // Comments are dropped (matches the previous fast_html5ever sink).
        // We still need to return a Handle — make an orphan Fragment that
        // gets garbage-collected with the rest of the tree on drop.
        self.tree.borrow_mut().orphan(Node::Fragment).id()
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Self::Handle {
        self.tree
            .borrow_mut()
            .orphan(Node::ProcessingInstruction(ProcessingInstruction {
                target: target.into_send().into(),
                data: data.into_send().into(),
            }))
            .id()
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let doctype = Doctype {
            name: name.into_send().into(),
            public_id: public_id.into_send().into(),
            system_id: system_id.into_send().into(),
        };
        self.tree
            .borrow_mut()
            .root_mut()
            .append(Node::Doctype(doctype));
    }

    fn append(&self, parent: &Self::Handle, child: NodeOrText<Self::Handle>) {
        let mut tree = self.tree.borrow_mut();
        let Some(mut parent_node) = tree.get_mut(*parent) else {
            return;
        };

        match child {
            NodeOrText::AppendNode(id) => {
                parent_node.append_id(id);
            }

            NodeOrText::AppendText(text) => {
                let can_concat = parent_node
                    .last_child()
                    .map_or(false, |mut n| n.value().is_text());

                let text = text.into_send().into();

                if can_concat {
                    if let Some(mut last_child) = parent_node.last_child() {
                        if let Node::Text(ref mut t) = *last_child.value() {
                            t.text.push_tendril(&text);
                            return;
                        }
                    }
                }
                parent_node.append(Node::Text(Text { text }));
            }
        }
    }

    fn append_before_sibling(
        &self,
        sibling: &Self::Handle,
        new_node: NodeOrText<Self::Handle>,
    ) {
        let mut tree = self.tree.borrow_mut();

        if let NodeOrText::AppendNode(id) = new_node {
            if let Some(mut node) = tree.get_mut(id) {
                node.detach();
            }
        }

        let Some(mut sibling_node) = tree.get_mut(*sibling) else {
            return;
        };
        if sibling_node.parent().is_none() {
            return;
        }

        match new_node {
            NodeOrText::AppendNode(id) => {
                sibling_node.insert_id_before(id);
            }
            NodeOrText::AppendText(text) => {
                let text = text.into_send().into();
                let can_concat = sibling_node
                    .prev_sibling()
                    .map_or(false, |mut n| n.value().is_text());

                if can_concat {
                    if let Some(mut prev_sibling) = sibling_node.prev_sibling() {
                        if let Node::Text(ref mut t) = *prev_sibling.value() {
                            t.text.push_tendril(&text);
                            return;
                        }
                    }
                }
                sibling_node.insert_before(Node::Text(Text { text }));
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &Self::Handle,
        prev_element: &Self::Handle,
        child: NodeOrText<Self::Handle>,
    ) {
        let has_parent = self
            .tree
            .borrow()
            .get(*element)
            .and_then(|n| n.parent())
            .is_some();
        if has_parent {
            self.append_before_sibling(element, child)
        } else {
            self.append(prev_element, child)
        }
    }

    fn remove_from_parent(&self, target: &Self::Handle) {
        if let Some(mut p) = self.tree.borrow_mut().get_mut(*target) {
            p.detach();
        }
    }

    fn reparent_children(&self, node: &Self::Handle, new_parent: &Self::Handle) {
        if let Some(mut p) = self.tree.borrow_mut().get_mut(*new_parent) {
            p.reparent_from_id_append(*node);
        }
    }

    fn add_attrs_if_missing(&self, target: &Self::Handle, attrs: Vec<Attribute>) {
        let mut tree = self.tree.borrow_mut();
        let Some(mut node) = tree.get_mut(*target) else {
            return;
        };
        let element = match *node.value() {
            Node::Element(ref mut e) => e,
            _ => return,
        };

        for attr in attrs {
            element
                .attrs
                .entry(attr.name)
                .or_insert(attr.value.into_send().into());
        }
    }

    fn get_template_contents(&self, target: &Self::Handle) -> Self::Handle {
        let tree = self.tree.borrow();
        // Defensive: fall back to the document root if the parser hands
        // us a non-template handle. Should never happen under the parser
        // invariants but we'd rather degrade than panic.
        tree.get(*target)
            .and_then(|n| n.first_child())
            .map(|c| c.id())
            .unwrap_or_else(|| tree.root().id())
    }
}
