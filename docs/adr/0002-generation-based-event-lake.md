# Catalog the event lake as generation-stamped files

Queries read a leased snapshot of the files catalogued in `projections.db`
(`lake_files`). Each file records the generation that created it and the one
that retired it, so publication and compaction cost O(files touched) and a
query can never race the deletion of a file it is reading: retired files are
unlinked only after the last lease on them is released. (The first design
replaced the complete manifest on every publish; it collapsed ingest to 684
events/s after 280k events.)
