class Distribution:

    def read(self) -> bytes:
        raise NotImplementedError

    @property
    def py_version(self) -> str:
        return "any"
