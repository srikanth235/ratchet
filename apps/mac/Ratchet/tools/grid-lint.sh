#!/usr/bin/env bash
# Two lints over every Swift file of Sources (Tokens.swift and Grid.swift included; only the accent file Theme.swift is skipped).
#
# 1. Grid: spacing numbers off the 4 pt grid, views nudged with offset or position, and size tokens
#    (`static let name: CGFloat = 9`) off the grid. Allowed numbers: 0, 1, 2, and multiples of 4.
#    Better than a number: a token (Space.lg, Dim.radiusButton, Layout.sheetWidth).
# 2. Kit: raw colors outside Tokens.swift (Color(red:), Color(hex:), Palette.), and SF Symbol names outside DesignKit/.
#
# Usage: tools/grid-lint.sh [folder]      (default: Sources)
# Silence one line with a reason: // grid: ok <why>     (kit rule: // kit: ok <why>)
# Portable: runs under bash and zsh, on macOS and Linux (needs perl).
set -euo pipefail

if [ "${1:-}" != "" ]; then
  dir=$1
else
  here=$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)
  dir="$here/../Sources"
fi

find "$dir" -name '*.swift' -not -name 'Theme.swift' -print0 | xargs -0 perl -ne '
  BEGIN { $count = 0 }
  my $file = $ARGV;
  my $line = $_;
  my $trim = $_; $trim =~ s/^\s+//;
  if ($line !~ /grid: ok/ && $line !~ /^\s*\/\//) {
    my @bad;
    while ($line =~ /(?:\.padding\((?:\.\w+|\[[^\]]*\])?,?\s*|spacing:\s*|(?:min|max|ideal)?(?:[Ww]idth|[Hh]eight):\s*|cornerRadius:\s*)(-?\d+(?:\.\d+)?)\b/g) {
      my $n = $1;
      push @bad, $n unless $n == 0 || $n == 1 || $n == 2 || ($n == int($n) && $n % 4 == 0);
    }
    # Size tokens: static let name: CGFloat = 9
    if ($line =~ /static let \w+:\s*CGFloat\s*=\s*(-?\d+(?:\.\d+)?)\b/) {
      my $n = $1;
      push @bad, $n unless $n == 0 || $n == 1 || $n == 2 || ($n == int($n) && $n % 4 == 0);
    }
    push @bad, "offset" if $line =~ /\.offset\(/;
    push @bad, "position" if $line =~ /\.position\(/;
    if (@bad) { $count++; print "$file:$.: off grid (", join(", ", @bad), "): $trim"; }
  }
  if ($line !~ /kit: ok/ && $line !~ /^\s*\/\//) {
    if ($file !~ /DesignKit\/Tokens\.swift$/ && $line =~ /Color\((?:red|hex):|Palette\./) {
      $count++; print "$file:$.: raw color outside Tokens.swift (use Ink): $trim";
    }
    if ($file !~ /\/DesignKit\// && $line =~ /systemName:/) {
      $count++; print "$file:$.: SF Symbol outside DesignKit (use KitIcon): $trim";
    }
  }
  close ARGV if eof;
  END { if ($count) { print "FAIL grid-lint: $count lines off the grid or the kit\n"; exit 1 } print "PASS grid-lint\n" }'
