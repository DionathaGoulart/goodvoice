//! What the transport has actually carried, as opposed to what was read off it.
//!
//! Every other instrument in this repo counts media *above* the socket: the
//! screen sink counts access units, the mixer counts samples, `bin/soak` reads
//! Windows' per-process IO counters. None of them can answer plan.md §7.9,
//! which asks whether Cloudflare keeps sending video after the viewer window
//! closes — because closing the viewer is exactly what stops this client
//! reading the track (`session::reconcile_watch`), so anything downstream of
//! the read goes to zero by construction and proves nothing. Windows' counters
//! cannot see it either: 5.2 / 5.4 / 5.6 kB/s across never-opened, open and
//! closed-again is the whole process, WebSocket and all, at a resolution that
//! a 720p share disappears into.
//!
//! webrtc's demuxer counts every datagram that arrives before anything has
//! decided what it is, and its RTP endpoint counts every packet it can name an
//! SSRC for, whether or not a receiver is draining that track. That is the
//! seam this module exposes. [`Wire`] is a snapshot of both; two snapshots and
//! the seconds between them are a bandwidth.

use rtc::rtp_transceiver::{rtp_sender::RtpCodecKind, SSRC};
use webrtc::peer_connection::{RTCStatsReport, RTCStatsReportEntry};

/// Everything one peer connection has carried since it opened.
///
/// Cumulative, never a rate: a call [`Wire::since`] on an earlier snapshot to
/// get one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wire {
    /// Every datagram in and out, headers and STUN and DTLS included.
    pub transport: Transport,
    /// One entry per inbound RTP stream the endpoint has seen, by SSRC.
    pub inbound: Vec<Inbound>,
}

/// The datagram counters, counted where the packets arrive rather than where
/// they are understood.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Transport {
    pub packets_received: u64,
    pub bytes_received: u64,
    pub packets_sent: u64,
    pub bytes_sent: u64,
}

/// One inbound RTP stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    pub ssrc: SSRC,
    /// Whether this stream is somebody's screen rather than their microphone.
    pub video: bool,
    /// The m-section it arrived on, as the SDP numbers them.
    pub mid: String,
    pub packets_received: u64,
    /// Payload only — [`Transport::bytes_received`] is the one that includes
    /// headers.
    pub bytes_received: u64,
}

impl Transport {
    /// This snapshot minus an earlier one.
    ///
    /// Saturating: a peer connection that was rebuilt between the two
    /// snapshots restarts its counters, and a reconnect mid-measurement should
    /// read as zero rather than as several exabytes.
    #[must_use]
    pub const fn since(self, earlier: Self) -> Self {
        Self {
            packets_received: self
                .packets_received
                .saturating_sub(earlier.packets_received),
            bytes_received: self.bytes_received.saturating_sub(earlier.bytes_received),
            packets_sent: self.packets_sent.saturating_sub(earlier.packets_sent),
            bytes_sent: self.bytes_sent.saturating_sub(earlier.bytes_sent),
        }
    }
}

impl Wire {
    /// The payload bytes of every video stream, added up.
    ///
    /// A room has one share at a time (`session::reconcile_watch` picks the
    /// first sharer it finds), so in practice this is that one track — but it
    /// is summed rather than found, because a stream that has stopped is still
    /// in the report with its final total.
    #[must_use]
    pub fn video_bytes(&self) -> u64 {
        self.stream_bytes(true)
    }

    /// The payload bytes of every audio stream, added up.
    #[must_use]
    pub fn audio_bytes(&self) -> u64 {
        self.stream_bytes(false)
    }

    fn stream_bytes(&self, video: bool) -> u64 {
        self.inbound
            .iter()
            .filter(|stream| stream.video == video)
            .map(|stream| stream.bytes_received)
            .sum()
    }

    /// Reads one out of a `getStats` report.
    ///
    /// The inbound streams come back sorted by SSRC so two snapshots line up
    /// row for row.
    pub(super) fn read(report: &RTCStatsReport) -> Self {
        let mut wire = Self::default();

        for entry in report.iter() {
            match entry {
                RTCStatsReportEntry::Transport(stats) => {
                    wire.transport = Transport {
                        packets_received: stats.packets_received,
                        bytes_received: stats.bytes_received,
                        packets_sent: stats.packets_sent,
                        bytes_sent: stats.bytes_sent,
                    };
                }
                RTCStatsReportEntry::InboundRtp(stats) => {
                    let stream = &stats.received_rtp_stream_stats;
                    wire.inbound.push(Inbound {
                        ssrc: stream.rtp_stream_stats.ssrc,
                        video: stream.rtp_stream_stats.kind == RtpCodecKind::Video,
                        mid: stats.mid.clone(),
                        packets_received: stream.packets_received,
                        bytes_received: stats.bytes_received,
                    });
                }
                _ => {}
            }
        }

        wire.inbound.sort_by_key(|stream| stream.ssrc);
        wire
    }
}

#[cfg(test)]
mod tests {
    use super::{Inbound, Transport, Wire};

    fn stream(ssrc: u32, video: bool, bytes: u64) -> Inbound {
        Inbound {
            ssrc,
            video,
            mid: if video {
                "1".to_owned()
            } else {
                "0".to_owned()
            },
            packets_received: bytes / 1000,
            bytes_received: bytes,
        }
    }

    #[test]
    fn a_delta_is_the_two_snapshots_subtracted() {
        let earlier = Transport {
            packets_received: 100,
            bytes_received: 40_000,
            packets_sent: 90,
            bytes_sent: 9_000,
        };
        let later = Transport {
            packets_received: 350,
            bytes_received: 190_000,
            packets_sent: 190,
            bytes_sent: 19_000,
        };

        let moved = later.since(earlier);
        assert_eq!(moved.packets_received, 250);
        assert_eq!(moved.bytes_received, 150_000);
        assert_eq!(moved.packets_sent, 100);
        assert_eq!(moved.bytes_sent, 10_000);
    }

    #[test]
    fn a_reconnect_resets_the_counters_and_reads_as_zero() {
        // The peer connection was rebuilt between the snapshots, so the later
        // one is *smaller*. That is a measurement to throw away, not a
        // negative bandwidth wrapped into u64::MAX.
        let earlier = Transport {
            bytes_received: 900_000,
            ..Transport::default()
        };
        let later = Transport {
            bytes_received: 12_000,
            ..Transport::default()
        };

        assert_eq!(later.since(earlier), Transport::default());
    }

    #[test]
    fn video_and_audio_are_added_up_separately() {
        let wire = Wire {
            transport: Transport::default(),
            inbound: vec![
                stream(11, false, 8_000),
                stream(22, true, 250_000),
                stream(33, false, 7_500),
            ],
        };

        assert_eq!(wire.video_bytes(), 250_000);
        assert_eq!(wire.audio_bytes(), 15_500);
    }

    #[test]
    fn a_stopped_stream_still_counts_what_it_carried() {
        // §7.9's whole question is what a track does *after* nobody is reading
        // it. Its row stays in the report with its final total, and dropping
        // rows that stopped moving would erase the answer.
        let wire = Wire {
            transport: Transport::default(),
            inbound: vec![stream(22, true, 250_000)],
        };
        assert_eq!(wire.video_bytes(), 250_000);
    }
}
