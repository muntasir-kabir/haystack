# Haystack icon assets

`../../haystack-icon.svg` is the canonical full-colour application mark. Run
`./scripts/export-icons.sh` after changing it. The command regenerates runtime
PNGs, package PNGs, 16–1024px previews, and the multi-resolution Windows ICO.
`cargo packager --formats app,dmg` converts those same PNGs into the ICNS in
the macOS bundle, which keeps the bundle icon on the existing packaging path.

`studies/haystack-h-bars.svg` is the rejected H-from-bars alternative. Its
letter-like symmetry weakens the log-stack silhouette at small sizes.
`studies/haystack-stack-needle.svg` is the selected stack-and-needle study.
The former magnifying-glass artwork is not retained because it fails the
single-metaphor requirement.

`src/ui/icons/brand-mark.svg` is the separate full-colour in-app mark. It is
not part of the 24px action-icon catalog, keeping toolbar identity distinct from
file actions and active-file affordances.
