# Soundboard media transport design

Status: proposed for voice#10; not implemented.

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

The signaling and SFU contracts currently model exactly one audio SSRC per
participant. A soundboard track therefore needs a separately identified SSRC,
its own media section, publisher/subscriber mapping and lifecycle. Additive
negotiation must preserve legacy clients: no receiver may silently treat the
soundboard stream as microphone audio, and a publisher must show a failure when
the call cannot safely support the feature. SSRCs must be nonzero, unique within
the room and released on departure; track replacement and reconnect must not
leave a stale receiver or bypass cryptor attachment. Call state changes, channel
switches and permission revocation must stop an active clip.

The CRUD and send API must authenticate the caller, enforce the guild and channel
permissions and verify current channel membership server-side, including when a
sound belongs to another guild. An API event must not carry sound content or
keys. The sound asset's access rules and the metadata exposed by a send event
need an explicit privacy review: DAVE protects audio content, not signaling
metadata. Limits on duration, decoded size, concurrent playback and request rate
must be enforced server-side as well as in the UI; malformed or excessive media
must not reach unbounded decoders. A user muting their microphone must not
accidentally unmute it when playing a sound.

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

This document records a proposed design, not an implementation or rollout.
