# Railway IaC

`railway.ts` is the single Railway configuration source. Install its pinned SDK
with `npm --prefix .railway ci` from the repository root and type-check it with
`npm --prefix .railway run check`.

Link the intended project/environment, run `railway config plan`, review the
changes, then apply the reviewed configuration with `railway config apply`.
This file owns the full project; merge existing resources before using it in a
shared project. The application deployment source targets `main`.

Secrets use `preserve()` and must be supplied as Railway service variables.
They are never loaded from a committed file. See the root README for first-time
setup, deployment, and the limits of the single-process setpoint cache.
