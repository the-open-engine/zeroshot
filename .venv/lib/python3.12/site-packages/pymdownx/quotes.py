"""
Extension for "enhanced" blockquotes.

This extension deviates from Python Markdown's original blockquote extension by:

- not grouping consecutive block quotes together.
- Allowing optional callout behavior that mimics GitHub or Obsidian.
"""
import re
import xml.etree.ElementTree as etree
from markdown.blockprocessors import BlockProcessor
from markdown.treeprocessors import Treeprocessor
from markdown import util
from markdown import Extension, Markdown
from markdown.blockparser import BlockParser
from typing import Any


class QuotesProcessor(BlockProcessor):
    """Process blockquotes."""

    RE = re.compile(r'(^|\n)[ ]{0,3}>[ ]?(.*)')
    RE_CALLOUT = re.compile(r'> *\[!([\w-]+(?: *\| *[\w-]+)*)]([-+])?(.*?)(?:\n|$)')

    def __init__(self, parser: BlockParser, config: dict[str, Any]) -> None:
        """Initialize."""

        super().__init__(parser)
        self.callouts = config['callouts']

    def test(self, parent: etree.Element, block: str) -> bool:
        """Test for block quote."""

        return bool(self.RE.search(block)) and not util.nearing_recursion_limit()

    def run(self, parent: etree.Element, blocks: list[str]) -> None:
        """Create blockquote."""

        block = blocks.pop(0)
        alert = []
        details = ''
        m = self.RE.search(block)
        if m:
            before = block[:m.start()]  # Lines before blockquote
            # Pass lines before blockquote in recursively for parsing first.
            self.parser.parseBlocks(parent, [before])
            # Remove `> ` from beginning of each line.
            lines = block[m.start():].split('\n')
            if lines and self.callouts:
                m2 = None
                index = 0
                for line in lines:
                    if line and line.strip() != '>':
                        m2 = self.RE_CALLOUT.match(line)
                        break
                    index += 1
                if m2:
                    alert = [x.strip() for x in m2.group(1).split('|')]
                    if m2.group(2):
                        details = 'open' if m2.group(2) == '+' else 'closed'
                    title = m2.group(3).strip() if m2.group(3) else ''
                    if not title:
                        title = alert[0].title()
                    lines[index] = ''
                    lines.insert(index, title)
                if alert:
                    alert[0] = alert[0].lower()
            block = '\n'.join([self.clean(l) for l in lines])

        # This is a new blockquote. Create a new parent element.
        attrs = {'data-alert': ' '.join(alert), 'data-alert-collapse': details} if alert else {}
        quote = etree.SubElement(parent, 'blockquote', attrs)

        # Recursively parse block with blockquote as parent.
        # change parser state so blockquotes embedded in lists use `p` tags
        self.parser.state.set('blockquote')
        self.parser.parseChunk(quote, block)
        self.parser.state.reset()

    def clean(self, line: str) -> str:
        """Remove `>` from beginning of a line."""

        m = self.RE.match(line)
        if line.strip() == ">":
            return ""
        elif m:
            return m.group(2)
        else:
            return line


class QuotesTreeprocessor(Treeprocessor):
    """Convert "special" quotes to the common output format for Admonitions and Details."""

    def run(self, root: etree.Element) -> etree.Element:
        """Find and convert "special" blockquotes."""

        for b in root.iter('blockquote'):
            if b.attrib.get('data-alert'):
                collapse = b.attrib.get('data-alert-collapse', '')
                if collapse:
                    b.tag = 'details'
                    child = b.find('*')
                    if collapse == 'open':
                        b.attrib['open'] = 'open'
                    c = b.attrib.get('class', '').split(' ')
                    if child is not None and child.tag.lower() == 'p':
                        child.tag = 'summary'
                else:
                    b.tag = 'div'
                    child = b.find('*')
                    c = b.attrib.get('class', '').split(' ')
                    c.append('admonition')
                    if child is not None and child.tag.lower() == 'p':
                        c2 = child.attrib.get('class', '').split(' ')
                        c2.append('admonition-title')
                        child.attrib['class'] = ' '.join(_c for _c in c2 if _c)
                c.append(b.attrib.get('data-alert', ''))
                b.attrib['class'] = ' '.join(_c for _c in c if _c)
                del b.attrib['data-alert']
                del b.attrib['data-alert-collapse']
        return root


class QuotesExtension(Extension):
    """Add blockquotes extension to Markdown class."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        """Initialize."""

        self.config = {
            'callouts': [False, "Enable GitHub/Obsidian style callouts - Default: False"]
        }
        super().__init__(*args, **kwargs)

    def extendMarkdown(self, md: Markdown) -> None:
        """Add support for blockquotes."""

        md.registerExtension(self)
        config = self.getConfigs()
        md.parser.blockprocessors.register(QuotesProcessor(md.parser, config), "quote", 20)
        if config['callouts']:
            md.treeprocessors.register(QuotesTreeprocessor(md), 'quotes', 19.99)


def makeExtension(*args: Any, **kwargs: Any) -> Extension:
    """Return extension."""

    return QuotesExtension(*args, **kwargs)
