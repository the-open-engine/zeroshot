# Checking class attributes

Class attributes are similar to function arguments. They look like this:

```python
class MyPet:
    name: str
    age_in_months: int
    weight_in_kg: float
    is_very_cute_or_not: bool = True
```

Oftentimes we'd like to also document them in docstrings and have _pydoclint_
check them. It is controlled by the `--check-class-attributes` option (see
<https://jsh9.github.io/pydoclint/config_options.html>)

However, none of the mainstream docstring styles (Google, numpy, or Sphinx)
offers explicit guidelines on documenting class attributes. Therefore,
_pydoclint_ adopts the following stance (i.e., how to write docstrings that
pass _pydoclint_'s check)

- Document the class attributes under the "Attribute" section, and document the
  input arguments to `__init__()` under the "Parameters" (or "Args") section
- Separate the "Attribute" and "Parameters" sections in your docstring
- You can use a single docstring (under the class name) or two docstrings (one
  under the class name and the other under `__init__()`)
  - If you use two docstrings, please keep the "Attributes" section in the
    docstring under the class name

Here are some examples showing how to document class attributes in different
styles:

## 1. Numpy style

```python
class MyPet:
    """
    A class to hold information of my pet.

    Attributes
    ----------
    name : str
        Name of my pet
    age_in_months : int
        Age of my pet (unit: months)
    weight_in_kg : float
        Weight of my pet (unit: kg)
    is_very_cute_or_not : bool
        Is my pet very cute?  Or just cute?

    Parameters
    ----------
    airtag_id : int
        The ID of the AirTag that I put on my pet
    """
    name: str
    age_in_months: int
    weight_in_kg: float
    is_very_cute_or_not: bool = True

    def __init__(self, airtag_id: int) -> None:
        self.airtag_id = airtag_id
```

From this example, we can see a few things:

1. The class attributes should be put in a different docstring section than the
   argument passed into the class constructor (`__init__()`)
1. Both the class attributes and the input arguments to `__init__()` are in the
   same docstring. (This is assuming the _pydoclint_ option
   `--allow-init-docstring` is `False`)

If `--allow-init-docstring` is set to `True`, we can write two separate
docstrings like this:

```python
class MyPet:
    """
    A class to hold information of my pet.

    Attributes
    ----------
    name : str
        Name of my pet
    age_in_months : int
        Age of my pet (unit: months)
    weight_in_kg : float
        Weight of my pet (unit: kg)
    is_very_cute_or_not : bool
        Is my pet very cute?  Or just cute?
    """
    name: str
    age_in_months: int
    weight_in_kg: float
    is_very_cute_or_not: bool = True

    def __init__(self, airtag_id: int) -> None:
        """
        Initialize a class object.

        Parameters
        ----------
        airtag_id : int
            The ID of the AirTag that I put on my pet
        """
        self.airtag_id = airtag_id
```

## 2. Google style

```python
class MyPet:
    """
    A class to hold information of my pet.

    Attributes:
        name (str): Name of my pet
        age_in_months (int): Age of my pet (unit: months)
        weight_in_kg (float): Weight of my pet (unit: kg)
        is_very_cute_or_not (bool): Is my pet very cute?  Or just cute?

    Args:
        airtag_id (int): The ID of the AirTag that I put on my pet
    """
    name: str
    age_in_months: int
    weight_in_kg: float
    is_very_cute_or_not: bool = True

    def __init__(self, airtag_id: int) -> None:
        self.airtag_id = airtag_id
```

You can also use two separate docstrings (one for the class and one for
`__init__()`, similar to the Numpy style.)

## 3. Sphinx style

```python
class MyPet:
    """
    A class to hold information of my pet.

    .. attribute :: name
        :type: str

        Name of my pet

    .. attribute :: age_in_months
        :type: int

        Age of my pet (unit: months)

    .. attribute :: weight_in_keg
        :type: float

        Weight of my pet (unit: kg)

    .. attribute :: is_very_cute_or_not
        :type: bool

        Is my pet very cute?  Or just cute?

    :param airtag_id: The ID of the AirTag that I put on my pet
    :type airtag_id: int
    """
    name: str
    age_in_months: int
    weight_in_kg: float
    is_very_cute_or_not: bool = True

    def __init__(self, airtag_id: int) -> None:
        self.airtag_id = airtag_id
```
