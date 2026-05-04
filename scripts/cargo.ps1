# cargo.ps1 — wrap cargo with the toolchain + env this project needs on this machine.
#
# Background: this is an ARM64 Windows host but the project's whisper-rs C++
# build does not work natively for arm64 here. We use the x64 toolchain via
# vcvarsall + a custom toolchain file that pins ggml's arch detection to x86.
# The full incantation is reproduced from the working build in the Sony UX570
# session log (3m44s clean release build, exit 0).
#
# Usage from PowerShell:
#   .\scripts\cargo.ps1 check
#   .\scripts\cargo.ps1 test --lib filename
#   .\scripts\cargo.ps1 build --release
#
# All arguments after the script path are forwarded to cargo verbatim.

param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$CargoArgs
)

$ErrorActionPreference = 'Continue'

$llvmBin   = 'C:\Users\varunramesh\LLVM-x64\bin'
$gitCmd    = 'C:\Program Files\Git\cmd'
$vcvars    = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvarsall.bat'
$toolchain = 'C:\Users\varunramesh\x64-toolchain.cmake'
$repoDir   = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

foreach ($p in @($llvmBin, $vcvars, $toolchain)) {
    if (-not (Test-Path $p)) {
        throw "Required path missing: $p"
    }
}

# Quote each cargo arg for cmd.exe.
$argString = ($CargoArgs | ForEach-Object { '"' + ($_ -replace '"', '\"') + '"' }) -join ' '

# In cmd.exe, `set FOO=value && next` puts a trailing space in FOO. We must
# chain set commands with no space before &&. The first `call vcvarsall` and
# the final `cd` / `cargo` use ` && ` for readability since they don't `set`.
$setBlock = @(
    "set PATH=$gitCmd;$llvmBin;!USERPROFILE!\.cargo\bin;!PATH!",
    "set LIBCLANG_PATH=$llvmBin",
    "set CC=clang-cl",
    "set CXX=clang-cl",
    "set CXXFLAGS=/EHsc",
    "set CMAKE_GENERATOR=Ninja",
    "set CMAKE_GENERATOR_INSTANCE=",
    "set CMAKE_TOOLCHAIN_FILE=$toolchain",
    "set GGML_NATIVE=OFF"
) -join '&& '

$cmd = "call `"$vcvars`" x64 && $setBlock&& cd /d `"$repoDir`" && cargo +stable-x86_64-pc-windows-msvc $argString"

cmd.exe /v:on /c "$cmd" 2>&1
exit $LASTEXITCODE
