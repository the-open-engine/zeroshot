"""Parser for attributes in ReST-style docstrings"""

from dataclasses import dataclass
from typing import List, Optional, Tuple


@dataclass
class Attribute:
    """Represents a parsed attribute with name, type, and description."""

    name: str
    type: Optional[str] = None
    description: Optional[str] = None


def parse_attributes(docstring: str) -> Tuple[List[Attribute], List[int]]:
    """Parse attributes from a ReST-style docstring."""
    attributes = []
    lines = docstring.split("\n")

    current_attr_lines = []
    current_attr_line_nums = []
    inside_attribute_block = False

    all_line_nums_with_attr: List[int] = []

    for i, line in enumerate(lines):
        stripped_line = line.strip()

        if stripped_line.startswith(".. attribute ::"):
            if current_attr_lines:
                attrs, line_nums_with_actual_attr = parse_attribute_block(
                    current_attr_lines, current_attr_line_nums
                )
                attributes.append(attrs)
                all_line_nums_with_attr.extend(line_nums_with_actual_attr)
                current_attr_lines = []
                current_attr_line_nums = []

            inside_attribute_block = True
            current_attr_lines.append(line)
            current_attr_line_nums.append(i)
        elif inside_attribute_block:
            if not stripped_line and current_attr_lines:
                # Check if the next line is also blank indicating end of block
                if current_attr_lines[-1].strip() == "":
                    inside_attribute_block = False
                    attrs, line_nums_with_actual_attr = parse_attribute_block(
                        current_attr_lines, current_attr_line_nums
                    )
                    attributes.append(attrs)
                    all_line_nums_with_attr.extend(line_nums_with_actual_attr)
                    current_attr_lines = []
                    current_attr_line_nums = []

            current_attr_lines.append(line)
            current_attr_line_nums.append(i)
        elif stripped_line.startswith(":") and current_attr_lines:
            # End the current attribute block if a new param or similar
            # is detected
            inside_attribute_block = False
            attrs, line_nums_with_actual_attr = parse_attribute_block(
                current_attr_lines, current_attr_line_nums
            )
            all_line_nums_with_attr.extend(line_nums_with_actual_attr)
            attributes.append(attrs)
            current_attr_lines = []
            current_attr_line_nums = []

    if current_attr_lines:
        attrs, line_nums_with_actual_attr = parse_attribute_block(
            current_attr_lines, current_attr_line_nums
        )
        attributes.append(attrs)
        all_line_nums_with_attr.extend(line_nums_with_actual_attr)

    return attributes, all_line_nums_with_attr


def parse_attribute_block(
    lines: List[str],
    global_line_nums: List[int],
) -> Tuple[Attribute, List[int]]:
    """Parse a single attribute block from lines."""
    name = None
    type_ = None
    description = []
    description_started = False

    line_nums_with_actual_attr: List[int] = []
    lines_with_actual_attr: List[str] = []

    # Get the base indentation level from the first line
    base_indent_level = len(lines[0]) - len(lines[0].lstrip())

    for j, line in zip(global_line_nums, lines):
        stripped_line = line.strip()
        current_indent_level = len(line) - len(line.lstrip())

        if stripped_line.startswith(".. attribute ::"):
            name = stripped_line[len(".. attribute ::") :].strip()
            lines_with_actual_attr.append(line)
            line_nums_with_actual_attr.append(j)
        elif stripped_line.startswith(":type:"):
            type_ = stripped_line[len(":type:") :].strip()
            lines_with_actual_attr.append(line)
            line_nums_with_actual_attr.append(j)
        elif current_indent_level > base_indent_level:
            # Include in the description if it has greater indentation or
            # description has already started
            if stripped_line or description_started:
                description_started = True
                description.append(stripped_line)
                lines_with_actual_attr.append(line)
                line_nums_with_actual_attr.append(j)

    # Clean up the description, removing leading/trailing empty lines
    description_text = "\n".join(description).strip() if description else None

    attr = Attribute(name=name, type=type_, description=description_text)

    return attr, line_nums_with_actual_attr
