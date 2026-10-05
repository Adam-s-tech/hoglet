# Two threads: group-commit writer, budgeted publisher

The writer drains the bounded request queue, writes every waiting batch, pays
one fsync per group, then acknowledges the group; it seals the active WAL
segment each second. The publisher turns sealed segments into Parquet,
applies identity and catalog projections, advances the WAL checkpoint in one
SQLite transaction, then reclaims the WAL; it also compacts, retires files and
erases persons. Publication never sits in the ack path and is budgeted per
call, so erasure and shutdown are never starved by ingest. Recovery drops an
unacknowledged torn tail of the active segment and refuses to start on any
other damage.
