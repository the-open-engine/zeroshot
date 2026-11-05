# $Id: misc.py 10300 2026-02-06 09:09:27Z milde $
# Author: David Goodger <goodger@python.org>
# Copyright: This module has been placed in the public domain.

"""
Miscellaneous transforms.
"""

from __future__ import annotations

__docformat__ = 'reStructuredText'

from docutils import nodes
from docutils.transforms import Transform


class CallBack(Transform):

    """
    Inserts a callback into a document.  The callback is called when the
    transform is applied, which is determined by its priority.

    For use with `nodes.pending` elements.  Requires a ``details['callback']``
    entry, a bound method or function which takes one parameter: the pending
    node.  Other data can be stored in the ``details`` attribute or in the
    object hosting the callback method.
    """

    default_priority = 990

    def apply(self) -> None:
        pending = self.startnode
        pending.details['callback'](pending)
        pending.parent.remove(pending)


class ClassAttribute(Transform):

    """
    Move the "class" attribute specified in the "pending" node into the
    next visible element.
    """

    default_priority = 210

    def apply(self) -> None:
        pending = self.startnode
        for element in pending.findall(include_self=False, descend=False,
                                       siblings=True, ascend=True):
            if isinstance(element, (nodes.Invisible, nodes.system_message)):
                continue
            element['classes'] += pending.details['class']
            pending.parent.remove(pending)
            return

        error = self.document.reporter.error(
            'No suitable element following "%s" directive'
            % pending.details['directive'],
            nodes.literal_block(pending.rawsource, pending.rawsource),
            line=pending.line)
        pending.replace_self(error)


class Transitions(Transform):
    """
    Post-process <transition> elements.

    Move transitions at the end of sections up the tree.
    Warn on transitions at the beginning or end of the document or
    a section (ignoring title, decoration, or invisible elements),
    and after another transition.

    For example, transform this::

        <section>
            ...
            <transition>
        <section>
            ...

    into this::

        <section>
            ...
        <transition>
        <section>
            ...
    """

    default_priority = 830

    def apply(self) -> None:
        for node in self.document.findall(nodes.transition):
            self.visit_transition(node)

    def visit_transition(self, node) -> None:
        msg = ''
        if not isinstance(node.parent, (nodes.document, nodes.section)):
            self.warn('Transition only valid as child of <document> '
                      'or <section>.', node)
        else:
            try:
                node.validate_position()
            except nodes.ValidationError as e:
                msg = str(e)
        if 'may not end' in msg:
            # Move transition up the tree.
            sibling = node.parent  # get new predecessor node
            parent = sibling.parent
            while parent is not None:
                index = parent.index(sibling)
                if index < len(parent) - 1:
                    node.parent.remove(node)
                    parent.insert(index + 1, node)
                    break
                sibling = sibling.parent
                parent = sibling.parent
            else:
                self.warn('Transition at the end of the document.', node)
        if 'may not begin' in msg:
            self.warn(f'Transition at the start of the {node.parent.tagname}.',
                      node)
        elif 'may not directly follow' in msg:
            self.warn('At least one body element should separate transitions.',
                      node)

    def warn(self, msg, node) -> None:
        # create a warning message, insert it if valid
        warning = self.document.reporter.warning(msg, base_node=node)
        if 'nodes.Body' in repr(node.parent.content_model):
            node.parent.insert(node.parent.index(node)+1, warning)
