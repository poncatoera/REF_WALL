# Wall's sync server

One Cloudflare Worker with two SQLite Durable Objects, for the free Workers
plan: boards, pictures, videos and share links of one library. No R2, KV or
paid binding, so there is nothing to bill; past a free limit requests fail
until the next day. Uploads stop at `MAX_MEDIA_BYTES` (4 GB).

- `npm test` runs it on this computer (no Cloudflare account involved) and
  checks merging, paging, the upload cap and what each kind of link may do.
- The owner's key stays on the owner's PC; the server has its SHA-256
  (`OWNER_HASH`). Share links are kept as hashes too, and can be revoked.
- A link opens the whole library or one board with the boards inside it, to
  view or to edit. Anyone who has a link can use it until it's revoked.
- File bytes aren't checked against their hash here (that would cost more
  CPU than the free plan gives): apps check after downloading.
