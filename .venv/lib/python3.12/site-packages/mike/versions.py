import json
import re
from verspec.loose import LooseVersion as Version

from . import jsonpath


def _ensure_version(version):
    if not isinstance(version, Version):
        return Version(version)
    return version


class VersionInfo:
    def __init__(self, version, title=None, aliases=[], properties=None):
        self._check_version(str(version), 'version')
        for i in aliases:
            self._check_version(i, 'alias')

        version_name = str(version)
        self.version = _ensure_version(version)
        self.title = version_name if title is None else title
        self.aliases = set(aliases)
        self.properties = properties

        if str(self.version) in self.aliases:
            raise ValueError('duplicated version and alias')

    @classmethod
    def from_json(cls, data):
        return cls(data['version'], data['title'], data['aliases'],
                   data.get('properties'))

    def to_json(self):
        data = {'version': str(self.version),
                'title': self.title,
                'aliases': list(self.aliases)}
        if self.properties:
            data['properties'] = self.properties
        return data

    @classmethod
    def loads(cls, data):
        return cls.from_json(json.loads(data))

    def dumps(self):
        return json.dumps(self.to_json(), indent=2)

    @staticmethod
    def _check_version(version, kind):
        if ( not version or version in ['.', '..'] or
             re.search(r'[\\/]', version) ):
            raise ValueError('{!r} is not a valid {}'.format(version, kind))

    def __eq__(self, rhs):
        return (str(self.version) == str(rhs.version) and
                self.title == rhs.title and
                self.aliases == rhs.aliases and
                self.properties == rhs.properties)

    def __repr__(self):
        return '<VersionInfo({!r}, {!r}, {{{}}}{})>'.format(
            self.version, self.title, ', '.join(repr(i) for i in self.aliases),
            ', {!r}'.format(self.properties) if self.properties else ''
        )

    def update(self, title=None, aliases=[]):
        for i in aliases:
            self._check_version(i, 'alias')
        if title is not None:
            self.title = title

        aliases = set(aliases)
        if str(self.version) in aliases:
            raise ValueError('duplicated version and alias')

        added = aliases - self.aliases
        self.aliases |= aliases
        return added

    def get_property(self, expr, **kwargs):
        return jsonpath.get_value(self.properties, expr, **kwargs)

    def set_property(self, expr, value):
        self.properties = jsonpath.set_value(self.properties, expr, value)


class Versions:
    def __init__(self):
        self._data = {}

    @classmethod
    def from_json(cls, data):
        result = cls()
        for i in data:
            version = VersionInfo.from_json(i)
            version_str = str(version.version)
            result._ensure_unique_aliases(version_str, version.aliases)
            result._data[version_str] = version
        return result

    def to_json(self):
        return [i.to_json() for i in iter(self)]

    @classmethod
    def loads(cls, data):
        return cls.from_json(json.loads(data))

    def dumps(self):
        return json.dumps(self.to_json(), indent=2)

    def __iter__(self):
        def key(info):
            # Development versions (i.e. those without a leading digit) should
            # be treated as newer than release versions.
            return (0 if re.match(r'v?\d', str(info.version))
                    else 1, info.version)

        return (i for i in sorted(self._data.values(), reverse=True, key=key))

    def __len__(self):
        return len(self._data)

    def __getitem__(self, k):
        return self._data[str(k)]

    def find(self, identifier, strict=False):
        identifier = str(identifier)
        if identifier in self._data:
            return (identifier,)
        for k, v in self._data.items():
            if identifier in v.aliases:
                return (k, identifier)
        if strict:
            raise KeyError(identifier)
        return None

    def _ensure_unique_aliases(self, version, aliases, update_aliases=False):
        removed_aliases = []

        # Check whether `version` is already defined as an alias.
        key = self.find(version)
        if key and len(key) == 2:
            if not update_aliases:
                raise ValueError('version {!r} already exists'.format(version))
            removed_aliases.append(key)

        # Check whether any `aliases` are already defined.
        for i in aliases:
            key = self.find(i)
            if key and key[0] != version:
                if len(key) == 1:
                    raise ValueError(
                        'alias {!r} already specified as a version'.format(i)
                    )
                if not update_aliases:
                    raise ValueError(
                        'alias {!r} already exists for version {!r}'
                        .format(i, str(key[0]))
                    )
                removed_aliases.append(key)

        return removed_aliases

    def add(self, version, title=None, aliases=[], update_aliases=False):
        v = str(version)
        removed_aliases = self._ensure_unique_aliases(
            v, aliases, update_aliases
        )

        if v in self._data:
            self._data[v].update(title, aliases)
        else:
            self._data[v] = VersionInfo(version, title, aliases)

        # Remove aliases from old versions that we've moved to this version.
        for i in removed_aliases:
            self._data[i[0]].aliases.remove(i[1])

        return self._data[v]

    def update(self, identifier, title=None, aliases=[], update_aliases=False):
        key = self.find(identifier, strict=True)
        removed_aliases = self._ensure_unique_aliases(
            key[0], aliases, update_aliases
        )

        # Remove aliases from old versions that we've moved to this version.
        for i in removed_aliases:
            self._data[i[0]].aliases.remove(i[1])

        return self._data[key[0]].update(title, aliases)

    def _remove_by_key(self, key):
        if len(key) == 1:
            item = self._data[key[0]]
            del self._data[key[0]]
        else:
            item = key[1]
            self._data[key[0]].aliases.remove(key[1])
        return item

    def remove(self, identifier):
        key = self.find(identifier, strict=True)
        return self._remove_by_key(key)

    def difference_update(self, identifiers):
        keys = [self.find(i, strict=True) for i in identifiers]
        return [self._remove_by_key(i) for i in keys]
