$ErrorActionPreference = 'Stop'
$failed = $false

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    & cargo fmt --all --check
    $failed = $failed -or ($LASTEXITCODE -ne 0)
    & cargo clippy --workspace --all-targets -- -D warnings
    $failed = $failed -or ($LASTEXITCODE -ne 0)
    & cargo test --workspace -- --test-threads=2
    $failed = $failed -or ($LASTEXITCODE -ne 0)
}
finally {
    Pop-Location
}

exit ([int]$failed)
