#!/usr/bin/env python3
"""Oracle for the AD-1 citation grammar (doc/adr/0001-citation-syntax.md).

`rr` ships a hand-rolled `citation::decode` (std-only, no `regex` crate). This
script holds the *canonical regex* the decoder must conform to, and serves two
roles, both dependency-free (stdlib `re` only):

  --selfcheck   Run the adversarial table and assert the regex behaves as we
                expect. Exit 0 if every expectation holds, 1 otherwise. This is
                the executable proof that "where we use a regex, it does what we
                think it does."

  (default)     Read NUL-separated tokens from stdin; for each, print one line:
                  ACCEPT\t<matched>      if the WHOLE token is a citation
                  REJECT                 otherwise
                This is the reader/`decode` oracle (anchored fullmatch). The
                gated Rust differential test pipes its table + fuzz corpus here
                and compares ACCEPT/REJECT against `decode`'s Citation vs
                (Bare|Malformed). NUL-separated because adversarial tokens may
                contain newlines; NUL cannot appear in a valid UTF-8 CLI anchor.

  --scan        Like default, but prints every finditer span (scanner contract,
                for the future `rr search` slice), one match per line, records
                separated by a blank line.
"""
import re
import sys

# The single source of truth. Keep byte-identical to AD-1 "Decision outcome".
CANON = r'\[\[rr:(?:\\.|[^\\\]\[\t\n\r])*?\]\](?:[@~][0-9a-fA-F]{7,40})?'
PAT = re.compile(CANON)


def accept(s):
    m = PAT.fullmatch(s)
    return m.group(0) if m else None


def scan(s):
    return [m.group(0) for m in PAT.finditer(s)]


# Mirror citation::escape / citation::wrap so we can prove "everything the
# emitter produces is accepted by the oracle, and the span is the whole token".
def _escape(s):
    return s.replace('\\', '\\\\').replace('[', '\\[').replace(']', '\\]')


def wrap(s):
    return '[[rr:' + _escape(s) + ']]'


# (input, should_accept) — the reader contract. Expectations are our mental
# model of the grammar; --selfcheck proves the regex agrees with all of them.
READER_TABLE = [
    ('[[rr:a]]', True),
    ('[[rr:]]', True),
    (r'[[rr:a\]]', False),                 # odd backslash eats the first ]
    (r'[[rr:a\\]]', True),                 # even backslash -> real terminator
    (r'[[rr:a\]]]', True),                 # \] then ]] -> anchor a]
    ('[[rr:arr[0]]]', False),              # raw [ and ] must be escaped
    (r'[[rr:arr\[0\]]]', True),
    ('[[rr:a]b]]', False),                 # lone unescaped ] mid-body
    (r'[[rr:a\]b]]', True),
    ('[[rr:foo[[rr:bar]]', False),         # raw nested sentinel has unescaped [
    (r'[[rr:foo\[\[rr:bar]]', True),
    ('[[rr:a]]@a1b2c3d', True),
    ('[[rr:a]]~deadbeefdeadbeefdeadbeefdeadbeefdeadbeef', True),  # 40-hex
    ('[[rr:a]]@a1b2c3', False),            # 6 hex too short
    ('[[rr:a]]@' + 'a' * 41, False),       # 41 hex too long
    ('[[rr:a]]@', False),                  # empty pin
    ('[[rr:a]]@a1b2c3d!', False),          # trailing junk after pin
    ('[[rr:a]]@a1b2c3z', False),           # non-hex pin
    ('[[rr:a]]xyz', False),                # trailing junk, no pin
    ('[[rr:support@example.com]]@a1b2c3d', True),  # embedded @ + outside pin
    ('[[rr:a\tb]]', False),                # raw TAB
    ('[[rr:a\\\tb]]', True),               # escaped TAB accepted (\\. matches tab)
    ('[[rr:a\nb]]', False),                # raw LF
    ('[[rr:a\\\nb]]', False),              # escaped LF NOT accepted (\\. excludes LF)
    ('[[rr:café 日本語 🦀]]', True),        # multibyte anchor
    (' [[rr:a]]', False),                  # leading space
    ('[[rr:a]] ', False),                  # trailing space
    ('[[foo]]', False),                    # wrong sentinel
    ('[[RR:a]]', False),                   # case-sensitive
    ('[[rr:a', False),                     # unterminated
    (r'[[rr:a\\\]]', False),               # \\ + \] -> no terminator
]

# Scanner contract (substring), for the future rr search / rr enforce slice.
SCANNER_TABLE = [
    ('see [[rr:handler]] here', ['[[rr:handler]]']),
    ('[[rr:a]] and [[rr:b]]', ['[[rr:a]]', '[[rr:b]]']),
    ('[[rr:a]]xyz', ['[[rr:a]]']),
    ('[[rr:a]]@a1b2c3d done', ['[[rr:a]]@a1b2c3d']),
    ('`[[rr:x]]`', ['[[rr:x]]']),          # backtick-wrapped (AD-1 R1) still found
    ('[[rr:a]]@a1b2c3 rest', ['[[rr:a]]']),  # bad pin -> citation found, pin dropped
]

# AD-1's core justification: the rr: sentinel yields no false positives.
NO_FALSE_POSITIVES = [
    'The handler() calls my_module::handler and emails support@example.com.',
    'See AD-42 and pyproject.toml#[tool.poetry] name for config.',
    'arr[0] = x; let y = [[maybe]]; a wiki [[link]] here.',
    'Refs: http.go:42, ~/path/to/file, a@b, foo~bar.',
    'Markdown [text](url) and an unclosed [[rr but no colon, and [[rr: with no end',
    'rr:anchor and [rr:anchor] and [[ rr:spaced ]] are all not the sentinel',
]

# Anchors of every kind; everything wrap() emits must be accepted whole.
EMIT_ROUNDTRIP = [
    'a', '', 'a\\', 'a]', ']]', 'arr[0]', 'pyproject.toml#[tool.poetry] name',
    'Index build: the writer', 'support@example.com', 'foo[[rr:bar', 'café 🦀', 'AD-42',
]


def selfcheck():
    fails = []

    def check(desc, cond):
        print(f"  {'PASS' if cond else 'FAIL'}  {desc}")
        if not cond:
            fails.append(desc)

    print("== A. reader contract (anchored fullmatch) ==")
    for s, exp in READER_TABLE:
        check(f"accept({s!r}) -> {'OK' if exp else 'REJECT'}", (accept(s) is not None) == exp)

    print("\n== A'. emit<->oracle (wrap output accepted, span == whole) ==")
    for x in EMIT_ROUNDTRIP:
        w = wrap(x)
        check(f"accept(wrap({x!r})) == whole", accept(w) == w)

    print("\n== B. scanner contract (finditer spans) ==")
    for s, exp in SCANNER_TABLE:
        check(f"scan({s!r}) == {exp}", scan(s) == exp)

    print("\n== C. no false positives in ordinary prose/code ==")
    for line in NO_FALSE_POSITIVES:
        check(f"0 matches: {line[:48]!r}", scan(line) == [])

    print(f"\n==== {'ALL EXPECTATIONS HOLD' if not fails else f'{len(fails)} DIVERGENCE(S)'} ====")
    return 1 if fails else 0


def oracle(do_scan):
    data = sys.stdin.buffer.read().decode('utf-8', 'surrogatepass')
    for tok in data.split('\0'):
        if tok == '' and data.endswith('\0'):
            continue
        if do_scan:
            for m in scan(tok):
                print(m)
            print()  # blank line separates records
        else:
            m = accept(tok)
            print(f"ACCEPT\t{m}" if m is not None else "REJECT")


if __name__ == '__main__':
    if '--selfcheck' in sys.argv[1:]:
        sys.exit(selfcheck())
    oracle('--scan' in sys.argv[1:])
