"""
A grammar for a very small subset of JSONPath. This supports bare field names,
quoted field names, and indices/field names inside square brackets. The only
operator supported is ".". For example, this is a valid expression:

    foo."bar"[0]["baz"]

When setting values, you can add to the head or tail of a list via the `head`
or `tail` keywords, e.g.:

    foo[head]
"""

import pyparsing as pp

Deleted = object()


class _IndexKeyword:
    def __init__(self, name):
        self.__name = name

    def __repr__(self):
        return '`{}`'.format(self.__name)


head = _IndexKeyword('head')
tail = _IndexKeyword('tail')

head_keyword = pp.Keyword('head').set_parse_action(lambda _: head)
tail_keyword = pp.Keyword('tail').set_parse_action(lambda _: tail)

identifier = pp.Word(pp.alphas + '_-', pp.alphanums + '_-')
string = pp.QuotedString('"') | pp.QuotedString("'")
integer = pp.common.signed_integer.set_parse_action(lambda t: int(t[0]))
index = integer | head_keyword | tail_keyword

field = (identifier | string).set_parse_action(lambda t: t[0])
subfield = pp.Suppress('.') + field
subscript = pp.Suppress('[') + (index | string) + pp.Suppress(']')

expr = (((field | subscript) + (subfield | subscript)[...]) |
        pp.empty).set_parse_action(lambda t: [t.as_list()])
set_expr = (expr + pp.Suppress('=') + pp.Regex('.*'))


def parse(expression):
    return expr.parse_string(expression, parse_all=True)[0]


def parse_set(expression):
    return tuple(set_expr.parse_string(expression, parse_all=True))


def _check_step(data, step):
    if not isinstance(step, (str, int)):  # pragma: no cover
        raise TypeError('invalid step {!r}'.format(step))

    if ( (isinstance(step, str) and not isinstance(data, dict)) or
         (isinstance(step, int) and not isinstance(data, list)) ):
        raise TypeError('incompatible type for step {!r}'.format(step))


def get_value(data, expression, *, strict=False):
    if isinstance(expression, str):
        expression = parse(expression)

    for step in expression:
        _check_step(data, step)

        try:
            data = data[step]
        except (IndexError, KeyError):
            if strict:
                raise
            return None

    return data


def set_value(data, expression, value, *, strict=False):
    if value is Deleted:
        return delete_value(data, expression, strict=strict)

    if isinstance(expression, str):
        expression = parse(expression)

    if len(expression) == 0:
        return value
    else:
        step = expression[0]
        if isinstance(step, str):
            if data is None:
                data = {}
            elif not isinstance(data, dict):
                raise TypeError('incompatible type for key {}'.format(step))

            data[step] = set_value(data.get(step), expression[1:], value)
        elif isinstance(step, (int, _IndexKeyword)):
            if data is None:
                data = []
            elif not isinstance(data, list):
                raise TypeError('incompatible type for key {}'.format(step))

            if isinstance(step, int):
                if step > len(data) - 1:
                    data = data + [None] * (step - len(data) + 1)
                data[step] = set_value(data[step], expression[1:], value)
            elif len(expression) > 1:
                raise TypeError('{!r} only allowed as last step'.format(step))
            elif step is head:
                data.insert(0, value)
            else:  # step is tail
                data.append(value)

        else:  # pragma: no cover
            raise TypeError('unrecognized step type {}'.format(type(step)))

    return data


def delete_value(data, expression, *, strict=False):
    if isinstance(expression, str):
        expression = parse(expression)

    if not len(expression):
        return None
    curr_data = data
    for step in expression[:-1]:
        _check_step(curr_data, step)

        try:
            curr_data = curr_data[step]
        except (IndexError, KeyError):
            if strict:
                raise
            return data

    _check_step(curr_data, expression[-1])
    try:
        del curr_data[expression[-1]]
    except (IndexError, KeyError):
        if strict:
            raise

    return data
