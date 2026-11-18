# Require exclusive ownership of a voice node registration

Status: proposed for develop, 2026-11-18.

Signaling and its SFU own in-memory rooms and media sessions. Their advertised node identity
and endpoints therefore require affinity. Previously a second process using the same node ID
could overwrite its etcd key; the older process could later delete the replacement's key.

Keep the P8 key and value unchanged. Claim an absent key using an etcd transaction comparing
version zero, and refresh or release it only while its attached lease equals this process's
lease. A conflicting process remains unregistered and unready. `NodeRegistration` is also a
required application port: IDENTIFY refuses new sessions without current ownership, including
after the locally tracked lease deadline expires or the presence process is unavailable.
Existing media sessions continue during discovery trouble. New owners must wait for a
crashed owner's lease to expire. This is not migration of an existing voice room.

Each node still needs a dedicated WebSocket endpoint and a distinct reachable public
IP/UDP-port pair. Gateway allocation rejects ambiguous advertisements. A generic Service
balancing one node's endpoint across unrelated signaling/SFU pairs is unsupported.

Tests reproduce live-owner overwrite and stale-owner deletion before the change, verify
admission fencing, and exercise all compare-and-write transitions against real etcd.
