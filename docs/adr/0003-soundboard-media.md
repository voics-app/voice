# Soundboard media transport design

Status: proposed for voice#10; partial prototypes in draft PRs, not deployable.

A soundboard clip must remain separately controllable at each receiver while a
microphone continues to be audible. Mixing the clip into the microphone before
DAVE encryption preserves confidentiality, but makes receiver-side soundboard
mute and volume impossible. Injecting server-side audio would give the node
access to plaintext in an encrypted call. Neither approach satisfies all of
voice#10's criteria.

## Proposed route

A publisher decodes the authorized guild sound locally and sends it on a second,
distinct Opus track. The track is protected by the same DAVE frame encryption as
other outbound audio before leaving the client. The SFU forwards opaque packets;
it does not fetch the sound or hold group keys. Each receiver decrypts the track
and sends it through a separate soundboard gain/mute control before the output
mixer. The client must fail closed if it cannot attach the media cryptor in an
E2EE call. In particular it must never fall back to clear audio or mix into the
microphone when the separate track cannot be established.

The baseline signaling and SFU contracts model one audio SSRC per participant.
The draft adds an optional separately identified SSRC and defaults its SFU
publication capability to denied. A production route also needs
its own media section, publisher/subscriber mapping and lifecycle. Additive
negotiation must preserve legacy clients: no receiver may silently treat the
soundboard stream as microphone audio, and a publisher must show a failure when
the call cannot safely support the feature. SSRCs must be nonzero, unique within
the room and released on departure; track replacement and reconnect must not
leave a stale receiver or bypass cryptor attachment. Call state changes, channel
switches and permission revocation must stop an active clip. The authorized
capability must require both voice publication and USE_SOUNDBOARD. It must be
bound to the correct session and node, default false, and be revoked on a
trusted, ordered live control path. The gateway-to-signaling control path is
not implemented yet. Stage soundboard stays unavailable until that path and
the SFU agree on the authoritative stage status.

The CRUD and send API must authenticate the caller, enforce the guild and channel
permissions and verify current channel membership server-side, including when a
sound belongs to another guild. An API event must not carry sound content or
keys. The sound asset's access rules and the metadata exposed by a send event
need an explicit privacy review: DAVE protects audio content, not signaling
metadata. Limits on duration, decoded size, concurrent playback and request rate
must be enforced server-side as well as in the UI; malformed or excessive media
must not reach unbounded decoders. A user muting their microphone must not
accidentally unmute it when playing a sound.

## Shared capability contract (proposed, not yet wired)

The gateway is the authorization owner. It derives soundboard permission from
current membership, channel `SPEAK` and `USE_SOUNDBOARD` permissions and, for
stage channels, authoritative speaker status. The gateway and signaling node
must bind every update to the gateway session, target node and that node's
current registration lease. A single atomic capability update carries both
`can_publish_audio`, `can_publish_video` and `can_publish_soundboard`, a monotonically increasing
per-session revision, and `expires_at` in Unix milliseconds. Soundboard publication is
allowed only while **both** bits are true and the grant is unexpired. Absence
of the capability is denial; this differs intentionally from legacy microphone
behavior. The update is authenticated with the existing internal service
credential and routed to the lease-identified node's private control endpoint.

The signaling node serializes updates per session and acknowledges only after
the SFU confirms the whole update was applied. Equal-revision retries are
idempotent only when their contents match; older or conflicting updates cannot
restore rights. A revoke is applied before its acknowledgement. Unknown node,
lease, session or expired update is rejected. A lost response is ambiguous: the
caller must not assume a grant was applied. If the gateway cannot establish
that a revoke took effect, it treats the session as unsafe and disconnects it
where possible. A network partition cannot guarantee immediate revocation; the
SFU denies publication when the bounded grant expires. Expiry is measured with a monotonic deadline
at the enforcer, with an agreed maximum lifetime and clock-skew policy for any
absolute timestamp. Reconnect, SFU restart and registration-lease replacement
must start denied and must not replay an expired or old-lease grant. A fresh,
authoritative grant is required before publishing resumes.

The client must only enable playback after an authenticated node announcement
confirms the capability has been applied and media encryption is ready when
required. The API send authorization also checks current voice membership and
both publication rights; a send authorization does not itself open the SFU
gate. Permission, stage, membership, session and lease changes invalidate
pending client fetch/decode, stop active playback and trigger a server-side
revoke. A direct RTP sender cannot bypass this gate. The currently proposed
membership lookup in gateway#67 reports only `connected`, so it cannot be used
as proof of soundboard publication permission.

The shared proposal is recorded in docs#215 (`specs/2026-11-20-stage-instances.md`).

Contract tests must cover ordered grant, revoke and ACK, duplicate and stale
revisions, expiry, lost ACK, node/lease mismatch, SFU replay and a concurrent
revoke versus grant. Two real peers must demonstrate that soundboard packets
stop on revoke while permitted microphone audio remains independent; stage
listeners and clients without the capability never publish. The end-to-end
client test must also prove receiver soundboard mute and gain do not alter voice,
user mute covers both streams, and DAVE calls never fall back to clear media.

## Required verification before release

- Exercise two actual clients through signaling and SFU in clear and DAVE calls;
  verify the sound and microphone arrive independently, receiver soundboard mute
  and volume affect only the sound, and encrypted calls fail closed without a
  cryptor. Cover reconnect, late join, device change and channel leave.
- Test authenticated CRUD, permissions, cross-guild access, limits, send and
  event delivery, including denial after membership or permission changes.
- Check older/newer participant combinations and missing capabilities; a client
  must explain an unsupported call rather than silently send insecure audio.
- Verify the normal upload, settings, panel, send and mute/volume routes in the
  desktop application, and mobile behavior where supported.

The API draft proposes canonical 48 kHz, mono, 16-bit PCM WAV assets of at
most five seconds and eight sounds per guild. CRUD events contain metadata,
not audio. A gateway membership lookup is proposed in gateway#67; connected
alone does not establish permission to publish a sound.

This document records a proposed design and partial prototypes, not a complete
implementation or rollout.
