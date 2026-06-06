# Validates cross-references in the project's Markdown so a doc can never again
# claim something is "recorded in X" when X is missing or moved. Three checks:
#   1. Every KB-#### id mentioned has a matching knowledge-base/entries/KB-####.md
#   2. Every root-relative 'docs/...md' or 'knowledge-base/...md' path mentioned
#      (e.g. in prose or backticks) exists on disk.
#   3. Every Markdown link target ending in .md  [text](path.md)  resolves,
#      relative to the file it appears in (handles ../ and root-relative links).
# Historical snapshots under docs/superpowers/ (plans, specs) are excluded, as
# are build artifacts under target/.
# Usage: ./tools/check-references.ps1   (exits non-zero if any reference is broken)
$ErrorActionPreference = "Stop"

# Repo root = parent of this script's folder.
$root = Split-Path -Parent $PSScriptRoot

$files = Get-ChildItem -Path $root -Recurse -Filter *.md -File |
    Where-Object {
        $_.FullName -notmatch '[\\/]docs[\\/]superpowers[\\/]' -and
        $_.FullName -notmatch '[\\/]target[\\/]'
    }

$problems = [System.Collections.Generic.List[string]]::new()

foreach ($f in $files) {
    $rel    = [System.IO.Path]::GetRelativePath($root, $f.FullName)
    $dir    = $f.DirectoryName
    # @() so a single-line file yields a one-element array, not a scalar string
    # (indexing a scalar string returns characters, silently scanning nothing).
    $lines  = @(Get-Content -LiteralPath $f.FullName)
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line   = $lines[$i]
        $lineNo = $i + 1

        # 1) KB-#### id -> entries/KB-####.md must exist
        foreach ($m in [regex]::Matches($line, 'KB-(\d{4})')) {
            $id = $m.Value
            if (-not (Test-Path -LiteralPath (Join-Path $root "knowledge-base/entries/$id.md"))) {
                $problems.Add(("{0}:{1}: references {2} but knowledge-base/entries/{2}.md does not exist" -f $rel, $lineNo, $id))
            }
        }

        # 2) Root-relative docs/ or knowledge-base/ path mention -> must exist
        foreach ($m in [regex]::Matches($line, '(?:docs|knowledge-base)/[A-Za-z0-9_./\-]+\.md')) {
            $p = $m.Value
            if (-not (Test-Path -LiteralPath (Join-Path $root $p))) {
                $problems.Add(("{0}:{1}: references path '{2}' which does not exist" -f $rel, $lineNo, $p))
            }
        }

        # 3) Markdown link targets [text](target.md) -> resolve per containing file
        foreach ($m in [regex]::Matches($line, '\]\(([^)]+?\.md)(?:#[^)]*)?\)')) {
            $target = $m.Groups[1].Value
            if ($target -match '^[a-z]+://' -or $target.StartsWith('#')) { continue }  # URLs / anchors
            $base = if ($target.StartsWith('/')) { $root } else { $dir }
            $resolved = [System.IO.Path]::GetFullPath((Join-Path $base ($target.TrimStart('/'))))
            if (-not (Test-Path -LiteralPath $resolved)) {
                $problems.Add(("{0}:{1}: link target '{2}' does not resolve" -f $rel, $lineNo, $target))
            }
        }
    }
}

$problems = $problems | Select-Object -Unique

if ($problems.Count -gt 0) {
    Write-Host "Reference check FAILED ($($problems.Count) issue(s)):" -ForegroundColor Red
    $problems | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    exit 1
}

Write-Host "Reference check OK - all KB ids, paths, and Markdown links resolve." -ForegroundColor Green
