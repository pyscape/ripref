# Grammar Loader Benchmark Fixture

This file is a fixed, committed input for the grammar_loader benchmark. The
benchmark parses it with the tree-sitter-md grammar and extracts ATX heading
anchors, so the file is deliberately dense with headings of every level. It is
hand-authored and never regenerated, which is the whole point: a stable parse
target whose size does not drift when project documentation changes.

Everything here is plain ASCII prose. There is no em dash, no curly quote, and
no smart punctuation, so the repository ASCII gate stays green and the fixture
is exempt from the documentation prose linter.

## Overview

The reader walks a serialized index and answers two kinds of question. The
first is an exact anchor lookup. The second is a covering query that reports
every span enclosing a given line. Both run over the same forward section.

### Goals

The goal of this fixture is to be boring and predictable. It should change only
when a human edits it on purpose, and it should contain enough headings that
the query has real work to do on every iteration of the benchmark.

### Non Goals

This fixture does not try to mirror the live project documentation. It does not
need to read well as a guide. It only needs to be valid markdown with many
headings and enough prose between them to look like a real document to the
parser.

## Architecture

The system splits cleanly into a writer and a reader. The writer walks a tree,
parses each file, extracts anchors, sorts them, and serializes the result. The
reader maps the serialized file and answers queries without re-parsing source.

### The Writer Path

The writer is a single serial loop today. It walks the tree with the ignore
crate, reads each file, parses it with tree-sitter, and extracts anchors using
a per-language query. The extracted anchors are sorted by name before they are
written, because the reader relies on that order.

### The Reader Path

The reader opens the serialized file and maps it into memory. It validates the
bytes as text, parses a small header and a section table, and then serves
lookups. A warm mapping is the common case, since a build loop reads the index
many times while it stays resident.

### The Index Format

The index format is a flat sequence of sections. Each section has a name and a
length, and the body of each section is a run of records. The forward section
holds anchor records sorted by name. The paths section holds the file paths.

## Anchors

An anchor is a stable name for a span of source. The simplest anchor is the
whole file path, which spans the entire file. Richer anchors name a function, a
type, a method, or a heading, and they span only the lines of that construct.

### Anchor Names

An anchor name is a path-like string. For source code it reads like a module
path with the symbol appended. For markdown it reads like the heading text. The
name is what a lookup matches against, so it must be unique within a file.

### Anchor Spans

An anchor span is an inclusive line range. Spans can nest, which is why the
covering query can return more than one result. A method span sits inside a type
span, which sits inside the whole file span, and a covering query on a line in
the method reports all three.

### Anchor Density

A realistic tree averages about twelve anchors per file. Most of those come
from functions and methods, with a handful of types, a couple of constants, and
the single whole-file path anchor that the indexer always adds.

## Queries

There are two query shapes. An exact lookup takes an anchor name and returns
its span, or nothing if no such anchor exists. A covering query takes a file and
a line and returns every span that encloses that line, outermost first.

### Exact Lookup

An exact lookup is a binary search over the sorted forward section. The search
itself is logarithmic, but the current implementation rebuilds a record index
over the whole section on every call before it bisects, so the real cost grows
with the size of the index. A later version can bisect the raw bytes in place.

### Covering Query

A covering query is a linear scan over the whole forward section. It keeps every
span whose range includes the requested line. Because it scans everything, its
cost grows directly with the total anchor count, and the benchmark reports it as
a scan rate in anchors per second.

### Miss Handling

A lookup that finds nothing must be no more expensive than a lookup that finds
a match. The benchmark checks both, so a regression that makes the miss path
slower than the hit path is caught.

## Freshness

Freshness is a stat, not a hash. Before serving a query the reader checks
whether any source file is newer than the index. If nothing is newer, the index
is fresh and the cached answers stand. If something is newer, the index is
stale and the caller is told to rebuild.

### The Stat Walk

The freshness check is a walk that stats each path and keeps the newest
modification time. On a large tree this walk can dominate query latency, so it
is worth optimizing on its own. The benchmark isolates it from the rest of the
query.

### The Serial Reduction

The simplest freshness check is a serial reduction. It stats each path in turn
and keeps the maximum modification time. Missing files contribute nothing. This
is the baseline the parallel version is measured against.

### The Parallel Reduction

Above a threshold the walk splits across threads. Each thread reduces a chunk
to its newest time, and the chunk results are combined. The threshold is chosen
so that small trees, where threading would only add overhead, stay on the serial
path.

### The Git Short Circuit

When the tree is a clean git checkout, the newest modification time can be read
from the tree object instead of walking every file. This short circuit skips the
stat walk entirely, which is the cheapest possible freshness check.

## Grammars

A grammar tells the parser how to turn source into a tree. Each supported
language has a grammar, and each grammar has a query that names the anchor
kinds for that language. The markdown grammar is the one this fixture exercises.

### First Class Grammars

A first class grammar is compiled into the binary as a function pointer. Loading
it costs almost nothing, since there is no compilation at runtime. Most of the
languages the tool ships with are first class.

### Third Party Grammars

A third party grammar is loaded at runtime from a compiled module. Loading it
costs a one time compilation, which is far more than a function pointer lookup
but happens only once per process. This is the cost of supporting grammars that
are not built in.

### Grammar Queries

A grammar query is a small program that names the nodes to capture. The markdown
query captures the inline content of every ATX heading, which is why this
fixture is full of headings. Each heading the parser sees is one capture.

## Performance Notes

The numbers that matter are the ones that transfer between machines. Wall clock
time on one machine is inflated by background work that another machine does not
do, so throughput is the portable signal.

### Why Throughput

Throughput is reported as elements per second. For the writer it is files per
second. For the covering query it is anchors per second. These rates do not
depend on the absolute speed of the machine in the way a raw time does, so they
compare cleanly across runs and across hardware.

### Why Fixed Inputs

A benchmark that parses a moving target produces a moving number. If the input
grows or shrinks, the parse time moves with it, and the change looks like a
performance regression or improvement when nothing about the code changed. A
fixed input removes that source of drift, which is exactly why this file exists.

### Why Determinism

A benchmark with random inputs produces noisy numbers from run to run. Every
generator in the benchmark suite derives its data from indices rather than a
random source, so the same run produces the same corpus every time. This fixture
follows the same rule by being hand-authored and never regenerated.

## Correctness Guards

Every benchmark in the suite has a guard that fails loudly if the work it is
about to time turns out to be meaningless. A benchmark that times broken work
produces fast but worthless numbers, and a guard catches that before any timing
runs.

### The Extraction Guard

The writer benchmark builds its corpus once up front and asserts that anchor
extraction actually fired. A dead grammar would still emit one path anchor per
file, so the guard requires far more anchors than that floor before it trusts
the corpus.

### The Lookup Guard

The reader benchmark asserts that a known anchor resolves to exactly one span
and that a known absent anchor resolves to nothing. A broken synthesis that
produced an unsorted forward section would fail this guard instead of timing
garbage.

### The Parity Guard

The grammar benchmark asserts that the third party grammar extracts the same
anchors as the first class grammar. A stale or wrong compiled module would
produce a different count, and the guard refuses to trust any number measured
against it.

## Conventions

The benchmark suite follows a few conventions so the files read consistently and
the numbers stay comparable.

### Naming

Each temporary directory carries the process id and the scale in its name, so
concurrent runs and repeated runs never collide. Each generated file is named
from its index, so the corpus is reproducible.

### Cleanup

Each scale removes its temporary tree as soon as its measurements are done. The
writer is read only on the tree, so all of its measurements reuse one corpus
that is removed afterward. The reader writes one file per scale and removes it
once the scale is measured.

### Scales

Each benchmark runs at a small set of scales chosen to bracket the realistic
range. The smallest scale stands in for a small project, the middle scale for a
mid size one, and the largest scale makes any growth with size unmistakable.

## Glossary

A short list of the terms used above, gathered in one place for a reader who
arrives in the middle.

### Anchor

A stable name for a span of source. The unit a lookup matches and a covering
query returns.

### Span

An inclusive line range. Spans nest, which is what makes a covering query return
more than one result.

### Forward Section

The run of anchor records, sorted by name, that a lookup searches and a covering
query scans.

### Freshness

The check that decides whether the cached index still reflects the source. A
stat, not a hash.

### Grammar

The description of a language that lets the parser build a tree. Either compiled
in as a first class language or loaded at runtime as a third party module.

## Edge Cases

A few inputs deserve their own mention, because they are easy to get wrong and
easy to forget when a generator is changed.

### Empty Files

An empty file still contributes one anchor, its path. The walk does not skip it,
and the reader treats it like any other zero-length span. A generator that
dropped empty files would quietly change the anchor count.

### Hidden Entries

The walk skips hidden entries, the ones whose names begin with a dot. The
generated corpus never dot-prefixes a file it wants counted, because such a file
would be invisible to the walk and would not appear in the index at all.

### Nested Spans

Nested spans are the reason a covering query exists. The corpus deliberately
builds one genuine nest so the covering guard can require a depth of at least
three. Without that nest the guard would pass on a flat corpus and prove nothing.

### Duplicate Names

Two anchors with the same name in the same file would break an exact lookup,
since the lookup expects a single match. The generators derive every name from
an index, so duplicates never arise, and the guard would catch them if they did.

## Maintenance

A note for whoever changes the benchmark suite next.

### Changing A Scale

Changing a scale changes the numbers, so a scale is only changed when the
realistic range it brackets has moved. The smallest and largest scales exist to
show the shape of the curve, not just a single point, so keep at least one of
each end if you retune them.

### Changing Anchor Density

The two density constants are tuned so each corpus averages about twelve anchors
per file, matching a real tree. If you change one, change the other to match, or
the two benchmarks stop describing the same kind of corpus.

### Changing This Fixture

This fixture can grow or shrink, but every such change moves the parse number,
so change it only on purpose and note why. Keep it ASCII only and keep it dense
with headings, since the heading query is the whole reason it is parsed.

## Closing

This file ends here. It is long enough to give the parser real work, dense
enough with headings to exercise the heading query, and fixed enough that the
number it produces means the same thing every time the benchmark runs.
