from argparse import *
import re as _re
import textwrap as _textwrap

_ArgumentParser = ArgumentParser
_Action = Action


# Add some simple wrappers to make it easier to specify shell-completion
# behaviors.

def _add_complete(argument, complete):
    if complete is not None:
        argument.complete = complete
    return argument


class ParagraphDescriptionHelpFormatter(HelpFormatter):
    def _fill_text(self, text, width, indent):
        # Re-fill text, but keep paragraphs. Why isn't this the default???
        return '\n\n'.join(_textwrap.fill(i, width) for i in
                           _re.split('\n\n', text.strip()))


class Action(_Action):
    def __init__(self, *args, complete=None, **kwargs):
        super().__init__(*args, **kwargs)
        _add_complete(self, complete)


class ArgumentParser(_ArgumentParser):
    @staticmethod
    def _wrap_complete(action):
        def wrapper(*args, complete=None, **kwargs):
            return _add_complete(action(*args, **kwargs), complete)

        return wrapper

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        for k, v in self._registries['action'].items():
            self._registries['action'][k] = self._wrap_complete(v)
