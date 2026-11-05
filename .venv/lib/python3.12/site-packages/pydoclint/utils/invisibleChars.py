def replaceInvisibleChars(text: str) -> str:
    """Replace invisible characters so that AST can correctly parse the code"""
    invisibleToSpace = {
        '\ufeff': ' ',  # Byte order mark: 0-width but might act as a separator
    }

    invisibleToEmpty = {
        '\u200b': '',  # Zero width space
        '\u200c': '',  # Zero width non-joiner
        '\u200d': '',  # Zero width joiner
        '\u2060': '',  # Word joiner
        '\u180e': '',  # Mongolian vowel separator
        '\u061c': '',  # Arabic letter mark
        '\u200e': '',  # Left-to-right mark
        '\u200f': '',  # Right-to-left mark
        '\u202a': '',  # Left-to-right embedding
        '\u202b': '',  # Right-to-left embedding
        '\u202c': '',  # Pop directional formatting
        '\u202d': '',  # Left-to-right override
        '\u202e': '',  # Right-to-left override
        '\u2061': '',  # Function application
        '\u2062': '',  # Invisible times
        '\u2063': '',  # Invisible separator
        '\u2064': '',  # Invisible plus
        '\u034f': '',  # Combining grapheme joiner
    }

    for char, replacement in {**invisibleToSpace, **invisibleToEmpty}.items():
        text = text.replace(char, replacement)

    return text
