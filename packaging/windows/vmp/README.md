# The Widevine VMP signature of the Windows player

Apple Music streams are Widevine-protected. On Windows (and macOS) Widevine only gives a licence to
a player whose program carries a **production VMP signature**. Uzumaki plays through castLabs
Electron, and castLabs signs it for free through their EVS service
(https://github.com/castlabs/electron-releases/wiki/EVS). This is the supported route: nothing is
extracted, patched or decrypted.

On Windows the signature is a separate file beside the program: `electron.exe.sig`. castLabs'
download ships one that is "valid for development only"; the production one replaces it.

## What is here

| File | What |
|---|---|
| `electron.exe.sig` | the production signature of `electron.exe` from `electron-v45.2.1+wvcus-win32-x64.zip` |
| `electron.exe.sha256` | the sha256 of that `electron.exe` (the signature fits these bytes only) |

It holds castLabs' certificate and a signature, no account data. It was made on a Mac
(2026-11-18): signing a Windows package from another system works.

```sh
# the zip named in bridge/node_modules/electron/checksums.json, checked against it
unzip electron-v45.2.1+wvcus-win32-x64.zip -d dist
python -m castlabs_evs.vmp sign-pkg dist      # needs the EVS account
python -m castlabs_evs.vmp verify-pkg dist    # "Signature is valid: streaming"; needs no account
```

## Why it is in the repository

The `windows` workflow builds test downloads from a branch. The EVS login is a secret of the
`build` environment, which only `v*` tags may use, and that rule stays. So a branch build can't
sign; it copies this file over the development one and **verifies** it (`verify-pkg` needs no
login). If castLabs Electron is updated (`bridge/package.json`), the bytes of `electron.exe` change
and the workflow fails at the sha256 check until both files here are made again.

A release build (a `v*` tag) should sign afresh with the `build` environment's secrets instead, as
the macOS job of `release.yml` does, and not use this copy.
