# $Id: standalone.py 10307 2026-03-28 18:54:14Z milde $
# Author: David Goodger <goodger@python.org>
# Copyright: This module has been placed in the public domain.

"""
Standalone file Reader for the reStructuredText markup syntax.
"""

from __future__ import annotations

__docformat__ = 'reStructuredText'

from docutils import frontend, readers
from docutils.transforms import frontmatter, references, misc

TYPE_CHECKING = False
if TYPE_CHECKING:
    from docutils.transforms import Transform


class Reader(readers.Reader):

    supported = ('standalone',)
    """Contexts this reader supports."""

    document = None
    """A single document tree."""

    settings_spec = (
        'Standalone Reader Options',
        None,
        (('Disable the promotion of a lone top-level section title to '
          'document title (and subsequent section title to document '
          'subtitle promotion; enabled by default).',
          ['--no-doc-title'],
          {'dest': 'doctitle_xform', 'action': 'store_false',
           'default': True, 'validator': frontend.validate_boolean}),
         ('Disable the bibliographic field list transform (enabled by '
          'default).',
          ['--no-doc-info'],
          {'dest': 'docinfo_xform', 'action': 'store_false',
           'default': True, 'validator': frontend.validate_boolean}),
         ('Activate the promotion of lone subsection titles to '
          'section subtitles (disabled by default).',
          ['--section-subtitles'],
          {'dest': 'sectsubtitle_xform', 'action': 'store_true',
           'default': False, 'validator': frontend.validate_boolean}),
         ('Deactivate the promotion of lone subsection titles.',
          ['--no-section-subtitles'],
          {'dest': 'sectsubtitle_xform', 'action': 'store_false'}),
         ))

    config_section = 'standalone reader'
    config_section_dependencies = ('readers',)

    def get_transforms(self) -> list[type[Transform]]:
        return super().get_transforms() + [
            references.Substitutions,
            references.SectionIDs,
            references.PropagateTargets,
            frontmatter.DocTitle,
            frontmatter.SectionSubTitle,
            frontmatter.DocInfo,
            references.AnonymousHyperlinks,
            references.IndirectHyperlinks,
            references.Footnotes,
            references.ExternalTargets,
            references.InternalTargets,
            references.DanglingReferences,
            misc.Transitions,
            ]
