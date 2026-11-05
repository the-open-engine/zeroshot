"""
Fancy lists in the style of Pandoc.

---
# A Python implementation of John Gruber's Markdown.

# Started by Manfred Stienstra (http://www.dwerg.net/).
# Maintained for a few years by Yuri Takhteyev (http://www.freewisdom.org).
# Currently maintained by Waylan Limberg (https://github.com/waylan),
# Dmitry Shachnev (https://github.com/mitya57) and Isaac Muse (https://github.com/facelessuser).

# Copyright 2007-2023 The Python Markdown Project (v. 1.7 and later)
# Copyright 2004, 2005, 2006 Yuri Takhteyev (v. 0.2-1.6b)
# Copyright 2004 Manfred Stienstra (the original version)

# License: BSD (see LICENSE.md for details).
---

Adapted to support "fancy" behavior by Copyright 2024 Isaac Muse.

Work in progress, not fully tested.
"""
from markdown.blockprocessors import BlockProcessor
from markdown.treeprocessors import Treeprocessor
from .blocks.block import Block
from .blocks import BlocksExtension
import xml.etree.ElementTree as etree
import re

ROMAN_MAP = {'I': 1, 'V': 5, 'X': 10, 'L': 50, 'C': 100, 'D': 500, 'M': 1000}

OL_STYLE = {
    '1': 'decimal',
    'a': 'lower-alpha',
    'A': 'upper-alpha',
    'i': 'lower-roman',
    'I': 'upper-roman'
}


def roman2int(s):
    """
    Convert Roman numeral to integer.

    Values should be validated before as no validation during conversion.
    """

    s = s.upper()

    # Initialize result
    total = 0
    i = 0
    while i < len(s):
        # Current index is less than the next, subtract current from next and sum value
        if i + 1 < len(s) and ROMAN_MAP[s[i]] < ROMAN_MAP[s[i + 1]]:
            total += ROMAN_MAP[s[i + 1]] - ROMAN_MAP[s[i]]
            i += 2
        # Sum the value
        else:
            total += ROMAN_MAP[s[i]]
            i += 1

    return total


class FancyOListProcessor(BlockProcessor):
    """Process fancy ordered list blocks."""

    TAG = 'ol'
    SIBLING_TAGS = ['ol']
    OL_TYPES = {
        'dot-decimal': '1',
        'paren-decimal': '1',
        'dot-roman': 'i',
        'paren-roman': 'i',
        'dot-ROMAN': 'I',
        'paren-ROMAN': 'I',
        'dot-alpha': 'a',
        'paren-alpha': 'a',
        'dot-ALPHA': 'A',
        'paren-ALPHA': 'A'
    }

    def __init__(self, parser, config):
        """Initialize."""

        super().__init__(parser)

        list_types = config['additional_ordered_styles']
        self.alpha_enabled = 'alpha' in list_types
        self.roman_enabled = 'roman' in list_types
        self.inject_style = config['inject_style']
        self.inject_class = config['inject_class']

        formats = ''

        if 'generic' in list_types:
            formats += r'| \#'

        if 'roman' in list_types:
            # Rules are similar to https://projecteuler.net/about=roman_numerals
            # We do not follow the "rule of 3": repeated values should not occur more than 3 times.
            # The above link suggests that repeats should be restricted such that lower denominations
            # do not equal or exceed X, C or M. We alter this to allow equaling to help mitigate
            # conflicts with alphabetical lists.
            formats += r'''
            | (?=[IVXLCDM]{2})
              M*
              (?:C[MD]|D(?:C{0,4}|C{5}\b)|(?:C{0,9}|C{10}\b))
              (?:X[CL]|L(?:X{0,4}|X{5}\b)|(?:X{0,9}|X{10}\b))
              (?:I[XV]|V(?:I{0,4}|I{5}\b)|(?:I{0,9}|I{10}\b))
            | (?=[ivxlcdm])
              m*
              (?:c[md]|d(?:c{0,4}|c{5}\b)|(?:c{0,9}|c{10}\b))
              (?:x[cl]|l(?:x{0,4}|x{5}\b)|(?:x{0,9}|x{10}\b))
              (?:i[xv]|v(?:i{0,4}|i{5}\b)|(?:i{0,9}|i{10}\b))
            '''

            if 'alpha' not in list_types:
                formats += r'''
                | [IVXLCDM](?=\)|\.[ ]{2})
                '''

        if 'alpha' in list_types:
            formats += r'''
            | [a-z]
            | [A-Z](?=\)|\.[ ]{2})
            '''

        # Detect an item list item.
        self.list_re = re.compile(
            r'^[ ]{0,%d}(?:(?:\d+%s)[).])[ ]+(.*)' % (self.tab_length - 1, formats),
            re.VERBOSE
        )

        # Detect items on secondary lines which can be of any list type.
        self.child_re = re.compile(
            r'^[ ]{0,%d}((?:(?:\d+%s)[).]|[-*+]))[ ]+(.*)' % (self.tab_length - 1, formats),
            re.VERBOSE
        )

        # Detect indented (nested) list items of any type.
        self.indent_re = re.compile(
            r'^[ ]{%d,%d}(?:(?:\d+%s)[).]|[-*+])[ ]+.*' % (self.tab_length, self.tab_length * 2 - 1, formats),
            re.VERBOSE
        )

        self.startswith = "1"

    def test(self, parent, block):
        """Test to see if block starts with a list."""

        return bool(self.list_re.match(block))

    def run(self, parent, blocks):
        """Process list items."""

        sibling = self.lastChild(parent)

        # Check for multiple items in one block and get the ordered list fancy type.
        items, fancy_type = self.get_items(sibling, blocks.pop(0), blocks)

        # Append list items that are under the sibling list if the list type matches
        if (
            sibling is not None and sibling.tag in self.SIBLING_TAGS and
            sibling.attrib.get('__fancylist', '') == fancy_type
        ):
            # Previous block was a list item, so set that as parent
            lst = sibling

            # Make sure previous item is in a `p` - if the item has text,
            # then it isn't in a `p`.
            if lst[-1].text:
                # Since it's possible there are other children for this
                # sibling, we can't just `SubElement` the `p`, we need to
                # insert it as the first item.
                p = etree.Element('p')
                p.text = lst[-1].text
                lst[-1].text = ''
                lst[-1].insert(0, p)

            # If the last item has a tail, then the tail needs to be put in a `p`
            # likely only when a header is not followed by a blank line.
            lch = self.lastChild(lst[-1])
            if lch is not None and lch.tail:
                p = etree.SubElement(lst[-1], 'p')
                p.text = lch.tail.lstrip()
                lch.tail = ''

            # Parse first block differently as it gets wrapped in a `p`.
            li = etree.SubElement(lst, 'li')
            self.parser.state.set('looselist')
            firstitem = items.pop(0)
            self.parser.parseBlocks(li, [firstitem])
            self.parser.state.reset()

        # This catches the edge case of a multi-item indented list whose
        # first item is in a blank parent-list item:
        # ```
        #     * * subitem1
        #         * subitem2
        # ```
        # see also `ListIndentProcessor`
        elif parent.tag in ['ol', 'ul']:
            lst = parent

        # This is a new, unique list so create parent with appropriate tag.
        else:
            if self.TAG == 'ol':
                # Correct the metadata of a forced list to now represent the actual content
                if sibling is not None and sibling.attrib.get('__fancylist', '').startswith('force'):
                    sibling.attrib['__fancylist'] = fancy_type
                    lst = sibling
                else:
                    attrib = {'type': self.OL_TYPES[fancy_type], '__fancylist': fancy_type}
                    if self.inject_style:
                        attrib['style'] = f"list-style-type: {OL_STYLE[attrib['type']]};"
                    if self.inject_class:
                        attrib['class'] = f"fancylists-{OL_STYLE[attrib['type']]}"
                    lst = etree.SubElement(
                        parent,
                        self.TAG,
                        attrib
                    )
            else:
                lst = etree.SubElement(parent, self.TAG)

            # Check if a custom start integer is set
            if self.startswith != '1' and not lst.attrib.get('start', ''):
                lst.attrib['start'] = self.startswith

        # Set the parse set to list
        self.parser.state.set('list')

        # Loop through items in block, recursively parsing each with the appropriate parent.
        for item in items:
            # Item is indented. Parse with last item as parent
            if item.startswith(' '*self.tab_length):
                self.parser.parseBlocks(lst[-1], [item])
            # New item. Create `li` and parse with it as parent
            else:
                li = etree.SubElement(lst, 'li')
                self.parser.parseBlocks(li, [item])

        # Reset the parse state
        self.parser.state.reset()

    def get_start(self, fancy_type, m):
        """Translate list convention into a logical start."""

        # Generic marker
        if m.group(1).startswith('#'):
            return '1'

        t = fancy_type.split('-')[1].lower()
        if t == 'decimal':
            return m.group(1)[:-1].lstrip('(')
        elif t == 'roman':
            return str(roman2int(m.group(1)[:-1]))
        elif t == 'alpha':
            return str(ord(m.group(1)[:-1].upper()) - 64)

    def get_fancy_type(self, m, first, fancy_type):
        """Get the fancy type for a given list item."""

        value = m.group(1)[:-1]
        sep = m.group(1)[-1]
        list_type = ''

        # Determine list type convention: _., _), (_)
        if sep == '.':
            list_type += 'dot-'
        elif sep == ')':
            list_type += 'paren-'
        else:
            return list_type, fancy_type

        # The first item will be forced to assume the sibling list's type
        if fancy_type.startswith('force'):
            ltype = fancy_type.split('-', 1)[1]
            # Make sure we aren't forcing an impossible scenario.
            # If everything looks sound, return the types
            if value == '#' or (
                (ltype.lower() == 'decimal' and value.isdigit()) or
                (
                    ltype.lower() == 'roman' and
                    self.roman_enabled and
                    value.isalpha() and
                    (len(value) > 2 or value.lower() in 'ivxlcdm')
                ) or
                (ltype.lower() == 'alpha' and self.alpha_enabled and len(value) == 1 and value.isalpha())
            ):
                fancy_type = list_type + fancy_type.split('-', 1)[1] if list_type else list_type
                return fancy_type, fancy_type

            # Ignore the force as it cannot be done
            fancy_type = ''

        # Determine numbering: numerical, roman numerical, alphabetic, or `#` numerical placeholder.
        if value == '#':
            list_type += fancy_type.split('-', 1)[1] if fancy_type else 'decimal'
        elif value.isdigit():
            list_type += 'decimal'
        elif len(value) == 1 and value.isalpha():
            if value.islower():
                in_roman = value in 'ivxlcdm'
                if (
                    self.alpha_enabled and (
                        not self.roman_enabled or (
                            first and (not in_roman or ((list_type + 'roman') != fancy_type and value != 'i'))
                        )
                    )
                ):
                    list_type += 'alpha'
                elif self.alpha_enabled and not first and ((list_type + 'alpha') == fancy_type or not in_roman):
                    list_type += 'alpha'
                else:
                    list_type += 'roman'
            elif value.isupper():
                in_roman = value in 'IVXLCDM'
                if (
                    self.alpha_enabled and (
                        not self.roman_enabled or (
                            first and (not in_roman or ((list_type + 'ROMAN') != fancy_type and value != 'I'))
                        )
                    )
                ):
                    list_type += 'ALPHA'
                elif self.alpha_enabled and not first and ((list_type + 'ALPHA') == fancy_type or not in_roman):
                    list_type += 'ALPHA'
                else:
                    list_type += 'ROMAN'
        elif value.isupper():
            list_type += 'ROMAN'
        elif value.islower():
            list_type += 'roman'

        return list_type, fancy_type

    def get_items(self, sibling, block, blocks):
        """Break a block into list items."""

        # Get ordered list fancy type
        fancy_type = ''
        if self.TAG == 'ol':
            if sibling is not None and sibling.tag in self.SIBLING_TAGS:
                fancy_type = sibling.attrib.get('__fancylist', '')
        fancy = fancy_type

        items = []
        rest = []
        for line in block.split('\n'):

            # We've found a list type that differs form the our current,
            # so gather the rest to be processed separately.
            if rest:
                rest.append(line)
                continue

            # Child list items
            m = self.child_re.match(line)
            if m:
                # This is a new list item check first item for the start index.
                # Also check for list items that differ from the first.
                fancy, fancy_type = self.get_fancy_type(m, not items, fancy)

                # We found a different fancy type, so handle these separately
                if items and fancy != fancy_type:
                    rest.append(line)
                    continue

                # Detect the integer value of first list item.
                # If we are already in a list, just grab that.
                if not items and self.TAG == 'ol':
                    self.startswith = self.get_start(fancy, m)
                fancy_type = fancy

                # Append to the list
                items.append(m.group(2))

            # Indented, possibly nested content
            elif self.indent_re.match(line):
                # Previous item was indented. Append to that item.
                if items[-1].startswith(' ' * self.tab_length):
                    items[-1] = '{}\n{}'.format(items[-1], line)
                # Other indented content
                else:
                    items.append(line)

            # Append non list items to previous list item.
            else:
                items[-1] = '{}\n{}'.format(items[-1], line)

        # Insert non-list items back into the blocks to be parsed later
        if rest:
            blocks.insert(0, '\n'.join(rest))

        return items, fancy_type


class FancyListBlock(Block):
    """Collapse code."""

    NAME = 'fancylists'
    ARGUMENT = True
    OL_TYPE = {
        '1': 'decimal',
        'a': 'alpha',
        'A': 'ALPHA',
        'i': 'roman',
        'I': 'ROMAN'
    }

    def on_init(self):
        """Handle initialization."""

        ordered_styles = self.config['additional_ordered_styles']
        self.inject_style = self.config['inject_style']
        self.inject_class = self.config['inject_class']
        self.roman_enabled = 'roman' in ordered_styles
        self.alpha_enabled = 'alpha' in ordered_styles

    def on_validate(self, parent):
        """Handle on validate event."""

        self.type = '1'
        self.start = None
        self.count = 0

        try:
            for a in self.argument.split():
                name, value = [x.strip() for x in a.split('=')]
                if name == 'type' and value in ['a', 'A', 'i', 'I', '1']:
                    if value.lower() == 'a' and not self.alpha_enabled:
                        raise ValueError('Alphabetical lists not enabled')
                    if value.lower() == 'i' and not self.roman_enabled:
                        raise ValueError('Alphabetical lists not enabled')
                    self.type = value
                elif name == 'start':
                    self.start = max(0, int(value))
                else:
                    raise ValueError('Not a valid option')
        except Exception:
            return False

        return True

    def on_create(self, parent):
        """Create the element."""

        # Create an ordered list that will guide the first list item's type
        attrib = {'type': self.type, '__fancylist': 'force-' + self.OL_TYPE[self.type]}
        if self.start is not None:
            attrib['start'] = str(self.start)
        if self.inject_style:
            attrib['style'] = f"list-style-type: {OL_STYLE[self.type]};"
        if self.inject_class:
            attrib['class'] = f"fancylists-{OL_STYLE[self.type]}"

        self.parent = parent
        self.ol = etree.SubElement(parent, 'ol', attrib)
        return parent

    def on_end(self, block):
        """On end."""

        # Remove the ordered list if empty.
        if not list(self.ol):
            self.parent.remove(self.ol)


class FancyUListProcessor(FancyOListProcessor):
    """Process unordered list blocks."""

    SIBLING_TAGS = ['ul']
    TAG = 'ul'

    def __init__(self, parser, config):
        """Initialize."""

        super().__init__(parser, config)
        self.list_re = re.compile(r'^[ ]{0,%d}[-+*][ ]+(.*)' % (self.tab_length - 1))


class FancyListTreeprocessor(Treeprocessor):
    """Clean up fancy list metadata."""

    def run(self, root):
        """Remove intermediate fancy list type metadata."""

        for ol in root.iter('ol'):
            if '__fancylist' in ol.attrib:
                del ol.attrib['__fancylist']
        return root


class FancyListExtension(BlocksExtension):
    """HTML Blocks Extension."""

    def __init__(self, *args, **kwargs):
        """Initialize."""

        self.config = {
            'additional_ordered_styles': [
                ['roman', 'alpha', 'generic'],
                "Specify the ordered list formats to add in addition to decimal.",
            ],
            'inject_style': [
                False,
                "Inject style attribute with the appropriate 'list-style-type'"
            ],
            'inject_class': [
                False,
                "Inject a class indicating the 'list-style-type'"
            ]
        }

        super().__init__(*args, **kwargs)

    def extendMarkdownBlocks(self, md, blocks):
        """Add Details to Markdown instance."""

        config = self.getConfigs()
        blocks.register(FancyListBlock, config)
        ol = FancyOListProcessor(md.parser, config)
        ul = FancyUListProcessor(md.parser, config)
        md.parser.blockprocessors.register(ol, 'olist', 40)
        md.parser.blockprocessors.register(ul, 'ulist', 30)
        md.treeprocessors.register(FancyListTreeprocessor(md), "olist-cleanup", 10)


def makeExtension(*args, **kwargs):
    """Return extension."""

    return FancyListExtension(*args, **kwargs)
