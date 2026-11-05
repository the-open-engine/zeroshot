"""Generic blocks extension."""
from __future__ import annotations
from markdown import Extension, Markdown
from markdown.blockprocessors import BlockProcessor
from markdown.treeprocessors import Treeprocessor
from markdown.blockparser import BlockParser
from markdown import util as mutil
from .. import util
import xml.etree.ElementTree as etree
import re
import yaml
import textwrap
from typing import cast, Any, TYPE_CHECKING

if TYPE_CHECKING:  # pragma: no cover
    from .block import Block

# Fenced block placeholder for SuperFences
FENCED_BLOCK_RE = re.compile(
    r'^([\> ]*){}({}){}$'.format(
        mutil.HTML_PLACEHOLDER[0],
        mutil.HTML_PLACEHOLDER[1:-1] % r'([0-9]+)',
        mutil.HTML_PLACEHOLDER[-1]
    )
)

# Block start/end
RE_START = re.compile(
    r'(?:^|\n)[ ]{0,3}(/{3,})[ ]*([\w-]+)[ ]*(?:\|[ ]*(.*?)[ ]*)?(?:\n|$)'
)

RE_END = re.compile(
    r'(?m)(?:^|\n)[ ]{0,3}(/{3,})[ ]*(?:\n|$)'
)

# Frontmatter patterns
RE_YAML_START = re.compile(r'(?m)^[ ]{0,3}(-{3})[ ]*(?:\n|$)')

RE_YAML_END = re.compile(
    r'(?m)^[ ]{0,3}(-{3})[ ]*(?:\n|$)'
)

RE_INDENT_YAML_LINE = re.compile(r'(?m)^(?:[ ]{4,}(?!\s).*?(?:\n|$))+')


class BlockEntry:
    """Track Block entries."""

    def __init__(self, block: Block, el: etree.Element, parent: etree.Element) -> None:
        """Block entry."""

        self.block: 'Block' = block
        self.el: etree.Element = el
        self.parent: etree.Element = parent
        self.hungry: bool = False


def get_frontmatter(string: str) -> dict[str, Any] | None:
    """
    Get frontmatter from string.

    YAML-ish key value pairs.
    """

    frontmatter = None

    try:
        frontmatter = yaml.safe_load(string)
        if frontmatter is None:
            frontmatter = {}
        if not isinstance(frontmatter, dict):
            frontmatter = None
    except Exception:
        pass

    return cast('dict[str, Any]', frontmatter)


def reindent(text: str, pos: int, level: int) -> list[str]:
    """Reindent the code to where it is supposed to be."""

    indented = []
    for line in text.split('\n'):
        index = pos - level
        indented.append(line[index:])
    return indented


def unescape_markdown(md: Markdown, blocks: list[str], is_raw: bool) -> list[str]:
    """Look for SuperFences code placeholders and other HTML stash placeholders and revert them back to plain text."""

    superfences = None
    try:
        from ..superfences import SuperFencesBlockPreprocessor
        processor = md.preprocessors['fenced_code_block']
        if isinstance(processor, SuperFencesBlockPreprocessor):
            superfences = processor.extension  # type: ignore[attr-defined]
    except Exception:
        pass

    new_blocks = []
    for block in blocks:
        new_lines = []
        for line in block.split('\n'):
            m = FENCED_BLOCK_RE.match(line)
            if m:
                key = m.group(2)

                # Extract SuperFences content
                indent_level = len(m.group(1))
                original = None
                if superfences is not None:
                    original, pos = superfences.stash.get(key, (None, None))
                    if original is not None:
                        code = reindent(original, pos, indent_level)
                        new_lines.extend(code)
                        superfences.stash.remove(key)

                # Extract other HTML stashed content
                if original is None and is_raw:
                    index = int(key.split(':')[1])
                    if index < len(md.htmlStash.rawHtmlBlocks):
                        original = md.htmlStash.rawHtmlBlocks[index]
                        if isinstance(original, etree.Element):
                            original = etree.tostring(original, encoding='unicode', method='html')
                        new_lines.append(original)

                # Couldn't find anything to extract
                if original is None:  # pragma: no cover
                    new_lines.append(line)
            else:
                new_lines.append(line)
        new_blocks.append('\n'.join(new_lines))

    return new_blocks


class BlocksTreeprocessor(Treeprocessor):
    """Blocks tree processor."""

    def __init__(self, md: Markdown, blocks: BlocksProcessor):
        """Initialize."""

        super().__init__(md)

        self.blocks = blocks

    def run(self, root: etree.Element) -> None:
        """Update tab IDs."""

        while self.blocks.inline_stack:
            entry = self.blocks.inline_stack.pop(0)
            entry.block.on_inline_end(entry.el)


class BlocksProcessor(BlockProcessor):
    """Generic block processor."""

    def __init__(self, parser: BlockParser, md: Markdown) -> None:
        """Initialization."""

        self.md = md

        # The Block classes indexable by name
        self.blocks: dict[str, type[Block]] = {}
        self.config: dict[str, dict[str, Any]] = {}
        self.empty_tags = {'hr',}
        self.block_level_tags = set(md.block_level_elements.copy())
        self.block_level_tags.add('html')

        # Block-level tags in which the content only gets span level parsing
        self.span_tags = {
            'address', 'dd', 'dt', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'legend', 'li', 'p', 'summary', 'td', 'th'
        }
        # Block-level tags which never get their content parsed.
        self.raw_tags = {'canvas', 'math', 'option', 'pre', 'script', 'style', 'textarea', 'code'}
        # Block-level tags in which the content gets parsed as blocks
        self.block_tags = set(self.block_level_tags) - (self.span_tags | self.raw_tags | self.empty_tags)
        self.span_and_blocks_tags = self.block_tags | self.span_tags

        super().__init__(parser)

        # Persistent storage across a document for blocks
        self.trackers: dict[str, dict[str, Any]] = {}
        # Currently queued up blocks
        self.stack: list[BlockEntry] = []
        # Blocks that should be processed after inline.
        self.inline_stack: list[BlockEntry] = []
        # When set, the assigned block is actively parsing blocks.
        self.working: BlockEntry | None = None
        # Cached the found parent when testing
        # so we can quickly retrieve it when running
        self.cached_parent: etree.Element | None = None
        self.cached_block: tuple['Block', str] | None = None

        # Used during the alpha/beta stage
        self.start = RE_START
        self.end = RE_END
        self.yaml_line = RE_INDENT_YAML_LINE

    def detab_by_length(self, text: str, length: int) -> tuple[str, str]:
        """Remove a tab from the front of each line of the given text."""

        newtext = []
        lines = text.split('\n')
        for line in lines:
            if line.startswith(' ' * length):
                newtext.append(line[length:])
            elif not line.strip():
                newtext.append('')  # pragma: no cover
            else:
                break
        if newtext:
            return '\n'.join(newtext), '\n'.join(lines[len(newtext):])
        return '\n'.join(lines[len(newtext):]), ''

    def register(self, b: type[Block], config: dict[str, Any]) -> None:
        """Register a block."""

        if b.NAME in self.blocks:
            raise ValueError(f'The block name {b.NAME} is already registered!')
        self.blocks[b.NAME] = b
        self.config[b.NAME] = config
        self.trackers[b.NAME] = {}

    def test(self, parent: etree.Element, block: str) -> bool:
        """Test to see if we should process the block."""

        # Are we hungry for more?
        if self.get_parent(parent) is not None:
            return True

        # Is this the start of a new block?
        m = self.start.search(block)
        if m:

            pre_text = block[:m.start()] if m.start() > 0 else None

            # Create a block object
            name = m.group(2).lower()
            if name in self.blocks:
                generic_block = self.blocks[name](len(m.group(1)), self.trackers[name], self, self.config[name])

                # Remove first line
                block = block[m.end():]

                # Get frontmatter and argument(s)
                options, the_rest = self.split_header(block, generic_block.length)
                arguments = m.group(3)

                # Options must be valid
                status = options is not None

                # Update the config for the Block
                if status:
                    status = generic_block._validate(parent, arguments, **options)  # type: ignore[arg-type]

                # Cache the found Block and any remaining content
                if status:
                    self.cached_block = (generic_block, the_rest)

                    # Any text before the block should get handled
                    if pre_text is not None:
                        self.parser.parseBlocks(parent, [pre_text])

                return status
        return False

    def _reset(self) -> None:
        """Reset."""

        self.stack.clear()
        self.inline_stack.clear()
        self.working = None
        self.trackers = {d: {} for d in self.blocks.keys()}

    def split_end(self, block: str, length: int) -> tuple[str | None, str | None, bool]:
        """Search for end and split the blocks while removing the end."""

        good = None
        bad = None
        end = False

        # Find the end of the Block
        m = None
        for match in self.end.finditer(block):
            if len(match.group(1)) == length:
                m = match
                break

        # Separate everything from before the "end" and after
        if m:
            temp = block[:m.start(0)]
            if temp:
                good = temp[:-1] if temp.endswith('\n') else temp
            end = True

            # Since we found our end, everything after is unwanted
            temp = block[m.end(0):]
            if temp:
                bad = temp
        else:
            # Gather blocks until we find our end
            good = block

        # Send back the new list of blocks to parse and note whether we found our end
        return good, bad, end

    def split_header(self, block: str, length: int) -> tuple[dict[str, Any] | None, str]:
        """Split, YAML-ish header out."""

        # Search for end in first block
        m = None
        blocks: list[str] = []
        for match in self.end.finditer(block):
            if len(match.group(1)) == length:
                m = match
                break

        # Move block ending to be parsed later
        if m:
            end = block[m.start(0):]
            blocks.insert(0, end)
            block = block[:m.start(0)]

        m = self.yaml_line.match(block)
        if m is not None:
            config = textwrap.dedent(m.group(0))
            blocks.insert(0, block[m.end():])
            if config.strip():
                return get_frontmatter(config), '\n'.join(blocks)

        blocks.insert(0, block)

        return {}, '\n'.join(blocks)

    def get_parent(self, parent: etree.Element) -> etree.Element | None:
        """Get parent."""

        # Returned the cached parent from our last attempt
        if self.cached_parent is not None:
            parent = self.cached_parent
            self.cached_parent = None
            return parent

        temp: etree.Element | None = parent
        while temp is not None:
            if not self.stack:
                break
            if self.stack[-1].hungry and self.stack[-1].parent is temp:
                self.cached_parent = temp
                return temp
            if temp is not None:
                temp = self.lastChild(temp)
        return None

    def is_raw(self, tag: etree.Element) -> bool:
        """Is tag raw."""

        return tag.tag in self.raw_tags

    def is_block(self, tag: etree.Element) -> bool:
        """Is tag block."""

        return tag.tag in self.block_tags

    def parse_blocks(self, blocks: list[str], current_parent: etree.Element) -> None:
        """Parse the blocks."""

        # Get the target element and parse
        while blocks and self.stack:
            b: str | None = blocks.pop(0)

            # Get the latest block on the stack
            # This is required to avoid some issues with `md_in_html`
            entry = self.stack[-1]
            target = entry.block.on_add(entry.el)

            # Since we are juggling the block parsers on the stack, the pipeline
            # has not fully adjusted list indentation, so look at how many
            # list item parents we have on the stack and adjust the content
            # accordingly.
            parent_map = {c: p for p in current_parent.iter() for c in p}
            # Only need to count lists between nested blocks
            parent = self.stack[-1].el if len(self.stack) > 1 else None
            li = 0
            while parent is not None:
                parent = parent_map.get(parent, None)
                if parent is not None:
                    if parent.tag in ('li', 'dd'):
                        li += 1
                    continue
                break

            b, a = self.detab_by_length(cast(str, b), li * self.tab_length)
            if a:
                blocks.insert(0, a)

            # Split out blocks we care about
            b, bad, end = self.split_end(b, entry.block.length)
            if bad is not None:
                blocks.insert(0, bad)

            # Parse the block under the given target
            if b is not None and target is not None:
                # Resolve modes
                mode = entry.block.on_markdown()
                if mode not in ('block', 'inline', 'raw'):
                    mode = 'auto'
                is_block = mode == 'block' or (mode == 'auto' and self.is_block(target))
                is_atomic = mode == 'raw' or (mode == 'auto' and self.is_raw(target))

                # We should revert fenced code in spans or atomic tags.
                # Make sure atomic tags have content wrapped as `AtomicString`.
                if is_atomic or not is_block:
                    child = list(target)[-1] if len(target) else None
                    text = target.text if child is None else child.tail
                    b = '\n\n'.join(unescape_markdown(self.md, [b], is_atomic)).strip('\n')

                    if text:
                        text += b if not b else '\n\n' + b
                    else:
                        text = b

                    if child is None:
                        target.text = mutil.AtomicString(text) if is_atomic else text
                    else:  # pragma: no cover
                        # TODO: We would need to build a special plugin to test this,
                        # as none of the default ones do this, but we have verified this
                        # locally. Once we've written a test, we can remove this.
                        child.tail = mutil.AtomicString(text) if is_atomic else text

                # Block tags should have content go through the normal block processor
                else:
                    self.parser.state.set('blocks')
                    working = self.working
                    self.working = entry
                    self.parser.parseChunk(target, b)
                    self.parser.state.reset()
                    self.working = working

            # Run "on end" event when we finish a block
            if end:
                entry.block._end(entry.el)
                self.inline_stack.append(entry)
                del self.stack[-1]

            # The Block does not or no longer accepts more content
            if target is None:  # pragma: no cover
                break

        if self.stack:
            self.stack[-1].hungry = True

    def capture_leaked_content(self, parent: etree.Element, entry: BlockEntry) -> None:
        """
        Capture leaked content.

        Old school, non-block admonitions, details,
        and content tabs strongly control where there content is inserted and
        can cause content leakage outside of the Blocks container.
        Look for such content and pull it back into the container if found.
        """

        last_child = self.lastChild(parent)
        if last_child is not None and last_child is not entry.el:
            target = entry.block.on_add(entry.el)
            parent.remove(last_child)
            target.append(last_child)

    def run(self, parent: etree.Element, blocks: list[str]) -> None:
        """Convert to details/summary block."""

        # Get the appropriate parent for this Block
        temp = self.get_parent(parent)
        if temp is not None:
            parent = temp

        # Did we find a new Block?
        if self.cached_block:
            # Get cached Block and reset the cache
            generic_block, block = self.cached_block
            self.cached_block = None

            # Discard first block as we've already processed what we need from it
            blocks.pop(0)
            if block:
                blocks.insert(0, block)

            # Ensure a "tight" parent list item is converted to "loose".
            if parent is not None and parent.tag in ('li', 'dd'):  # pragma: no cover
                text = parent.text
                if parent.text:
                    parent.text = ''
                    p = etree.SubElement(parent, 'p')
                    p.text = text

            # Create the block element
            el = generic_block._create(parent)

            # Push a Block entry on the stack.
            self.stack.append(BlockEntry(generic_block, el, parent))

            # Parse the text blocks under the Block
            self.parse_blocks(blocks, parent)

        else:
            for r in range(len(self.stack)):
                entry = self.stack[r]
                if entry.hungry and parent is entry.parent:

                    # Capture leaked content from old-school extensions: admonition, details, tabbed, etc.
                    self.capture_leaked_content(parent, entry)

                    # Get the target element and parse
                    entry.hungry = False
                    self.parse_blocks(blocks, parent)

                    break


class BlocksMgrExtension(Extension):
    """Add generic Blocks extension."""

    def extendMarkdown(self, md: Markdown) -> None:
        """Add Blocks to Markdown instance."""

        md.registerExtension(self)
        util.escape_chars(md, ['/'])
        self.extension = BlocksProcessor(md.parser, md)
        # We want to be right after list indentations are processed
        md.parser.blockprocessors.register(self.extension, "blocks", 89.99)

        tree = BlocksTreeprocessor(md, self.extension)
        md.treeprocessors.register(tree, 'blocks_on_inline_end', 19.99)

    def reset(self) -> None:
        """Reset."""

        self.extension._reset()


class BlocksExtension(Extension):
    """Blocks Extension."""

    def register_block_mgr(self, md: Markdown) -> BlocksProcessor:
        """Add Blocks to Markdown instance."""

        if 'blocks' not in md.parser.blockprocessors:
            ext = BlocksMgrExtension()
            ext.extendMarkdown(md)
            mgr = ext.extension
        else:
            mgr = cast('BlocksProcessor', md.parser.blockprocessors['blocks'])
        return mgr

    def extendMarkdown(self, md: Markdown) -> None:
        """Extend markdown."""

        mgr = self.register_block_mgr(md)
        self.extendMarkdownBlocks(md, mgr)

    def extendMarkdownBlocks(self, md: Markdown, block_mgr: BlocksProcessor) -> None:
        """Extend Markdown blocks."""
