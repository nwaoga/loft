# First open / Gatekeeper & SmartScreen

These builds are unsigned. macOS and Windows may warn you the first time you open Loft.
The app is not damaged; the warning is because the build has no Apple or Microsoft signature.

**macOS.** Gatekeeper may say Loft is from an unidentified developer, or that it is damaged
and should be moved to the Trash. After you copy the app into Applications, open Terminal
and run:

```bash
xattr -cr /Applications/Loft.app
```

If Loft is not in Applications, use its actual path instead of `/Applications/Loft.app`.
You can also right-click `Loft.app` and choose **Open**, then confirm.

**Windows.** If SmartScreen says "Windows protected your PC", choose **More info**, then
**Run anyway**.

If it still will not open, [open an issue](https://github.com/nwaoga/loft/issues) with your
operating system, CPU architecture, Loft version, and a screenshot of the warning. Do not
include private file paths or document contents.
