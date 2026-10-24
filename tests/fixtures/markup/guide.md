# Light guide

![logo](logo.png)

A short guide to **Light**, the IDE made of *modules*. It has `inline code`, a [link to the manual](manual.rst),
a [link to a section](#install) and a ~~struck~~ word.

> Light is an editor first.
> Everything else is a module.

## Install

1. Download the zip
2. Unpack it
3. Run `light.exe`

- [x] a done task
- [ ] an open task
  - nested item
  - another one

## Languages

| Module   | Files        | Run command |
|:---------|:------------:|------------:|
| Python   | `.py`        | `python3`   |
| C and C++| `.c`, `.cpp` | `cc`        |
| Lisp     | `.lisp`      | SBCL        |

## Code

```python
def greet(name):
    """Say hello."""
    return f"hello {name}"  # a comment
```

```lisp
(defun greet (name)
  "Say hello."
  (format nil "hello ~a" name))
```

```c
#include <stdio.h>
int main(void) { printf("hi\n"); return 0; }
```

> [!WARNING]
> Modules are code you download: read what they do.

---

## Usage

Paragraph one of the usage section. It goes on for a few lines so that the document is long enough
to need scrolling in the preview and in the editor, and so that the two sides have something to keep in step.

Paragraph two. More text, more text, more text, more text, more text, more text, more text, more text,
more text, more text, more text, more text, more text, more text, more text, more text.

### First topic

Text of the first topic.

### Second topic

Text of the second topic.

- one
- two
- three

### Third topic

Text of the third topic, with a bare link https://example.org/page and an email <me@example.org>.

## More

Line 1.
Line 2.
Line 3.

## Even more

Line 1.
Line 2.
Line 3.

## The end

Last paragraph.
