"""
B64.

An extension for Python Markdown.
Given an absolute base path, this extension searches for image tags,
and if the images are local, will embed the images in base64.

MIT license.

Copyright (c) 2014 - 2017 Isaac Muse <isaacmuse@gmail.com>

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software,
and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
"""
from markdown import Extension
from markdown.postprocessors import Postprocessor
from . import util
import os
import base64
import re

RE_SLASH_WIN_DRIVE = re.compile(r"^/[A-Za-z]{1}:/.*")

file_types = {
    (".png",): "image/png",
    (".jpg", ".jpeg"): "image/jpeg",
    (".gif",): "image/gif",
    (".svg",): "image/svg+xml",
}

RE_TAG_HTML = re.compile(
    r'''(?xus)
    (?:
        (?P<avoid>
            <\s*(?P<script_name>script|style)[^>]*>.*?</\s*(?P=script_name)\s*> |
            (?:(\r?\n?\s*)<!--[\s\S]*?-->(\s*)(?=\r?\n)|<!--[\s\S]*?-->)
        )|
        (?P<open><\s*(?P<tag>img))
        (?P<attr>(?:\s+[\w\-:]+(?:\s*=\s*(?:"[^"]*"|'[^']*'))?)*)
        (?P<close>\s*(?:\/?)>)
    )
    '''
)

RE_TAG_LINK_ATTR = re.compile(
    r'''(?xus)
    (?P<attr>
        (?:
            (?P<name>\s+src\s*=\s*)
            (?P<path>"[^"]*"|'[^']*')
        )
    )
    '''
)


def repl_path(m, base_path, root_path, restrict_path=True):
    """Replace path with b64 encoded data."""

    link = m.group(0)
    try:
        _, _, path, _, _, _, is_url, is_absolute = util.parse_url(m.group('path')[1:-1])
        if not is_url:
            path = util.url2path(path)

        if is_absolute:
            file_name = os.path.normpath(path)
        else:
            file_name = os.path.normpath(os.path.join(base_path, path))

        if restrict_path:
            filename = os.path.abspath(file_name)
            # If the absolute path is no longer under the specified base path, reject the file
            # Append `os.sep` so a sibling directory whose name shares a prefix
            # (e.g. `/x/docs` vs `/x/docs_evil`) cannot satisfy the check.
            if not filename.startswith(root_path + os.sep if not root_path.endswith(os.sep) else root_path):
                return link

        if os.path.exists(file_name):
            ext = os.path.splitext(file_name)[1].lower()
            for b64_ext in file_types:
                if ext in b64_ext:
                    with open(file_name, "rb") as f:
                        link = " src=\"data:{};base64,{}\"".format(
                            file_types[b64_ext],
                            base64.b64encode(f.read()).decode('ascii')
                        )
                    break
    except Exception:  # pragma: no cover
        # Parsing crashed and burned; no need to continue.
        pass

    return link


def repl(m, base_path, root_path, restrict_path=True):
    """Replace."""

    if m.group('avoid'):
        tag = m.group('avoid')
    else:
        tag = m.group('open')
        tag += RE_TAG_LINK_ATTR.sub(
            lambda m2: repl_path(m2, base_path, root_path, restrict_path),
            m.group('attr')
        )
        tag += m.group('close')
    return tag


class B64Postprocessor(Postprocessor):
    """Post processor for B64."""

    def run(self, text):
        """Find and replace paths with base64 encoded file."""

        base_path = os.path.abspath(self.config['base_path'])
        root_path = self.config['root_path']
        root_path = base_path if not root_path else os.path.abspath(root_path)
        restrict_path = self.config['restrict_path']
        text = RE_TAG_HTML.sub(lambda m: repl(m, base_path, root_path, restrict_path=restrict_path), text)
        return text


class B64Extension(Extension):
    """B64 extension."""

    def __init__(self, *args, **kwargs):
        """Initialize."""

        self.config = {
            'base_path': [
                ".",
                "Base path for b64 to use. Operates as restricted root directory and a relative anchor to resolve "
                "paths if `relative_path` is not defined - Default: \".\""
            ],
            'root_path': [
                "",
                "Root path to restrict links to if `base_path` is not sufficient. Ignored if not defined."
            ],
            'restrict_path': [
                True,
                "Restrict B64 paths such that they are under the base path (`base_path`); if `root_path` is provided, "
                "links must be under `root_path` instead - Default: True"
            ],
        }

        super().__init__(*args, **kwargs)

    def extendMarkdown(self, md):
        """Add base 64 tree processor to Markdown instance."""

        b64 = B64Postprocessor(md)
        b64.config = self.getConfigs()
        md.postprocessors.register(b64, "b64", 2)
        md.registerExtension(self)


def makeExtension(*args, **kwargs):
    """Return extension."""

    return B64Extension(*args, **kwargs)
