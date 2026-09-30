use crate::data_channel::RTCDataChannelId;
use crate::data_channel::internal::RTCDataChannelInternal;
use crate::data_channel::message::RTCDataChannelMessage;
use crate::data_channel::registry::DataChannelRegistry;
use crate::data_channel::state::RTCDataChannelState;
use crate::peer_connection::event::data_channel_event::RTCDataChannelEvent;
use crate::peer_connection::event::{RTCEventInternal, RTCPeerConnectionEvent};
use crate::peer_connection::message::internal::{
    ApplicationMessage, DTLSMessage, DataChannelEvent, RTCMessageInternal, TaggedRTCMessageInternal,
};
use crate::peer_connection::transport::dtls::role::RTCDtlsRole;
use crate::statistics::accumulator::RTCStatsAccumulator;
use log::{debug, warn};
use sansio::Protocol as _;
use sctp::PayloadProtocolIdentifier;
use shared::TransportContext;
use shared::error::{Error, Result};
use std::collections::VecDeque;
use std::time::Instant;

#[derive(Default)]
pub(crate) struct DataChannelHandlerContext {
    pub(crate) read_outs: VecDeque<TaggedRTCMessageInternal>,
    pub(crate) write_outs: VecDeque<TaggedRTCMessageInternal>,
    pub(crate) event_outs: VecDeque<RTCEventInternal>,
}

/// DataChannelHandler implements DataChannel Protocol handling
pub(crate) struct DataChannelHandler<'a> {
    ctx: &'a mut DataChannelHandlerContext,
    data_channels: &'a mut DataChannelRegistry,
    stats: &'a mut RTCStatsAccumulator,
    /// The DTLS role this endpoint negotiated, which RFC 8832 §6 turns into the parity of the
    /// stream ids assigned at `SCTPHandshakeComplete`. Resolved by the time an association
    /// exists, so the handler never has to guess it.
    dtls_role: RTCDtlsRole,
    /// The association's negotiated stream limit, bounding stream-id assignment.
    max_channels: u16,
}

impl<'a> DataChannelHandler<'a> {
    pub(crate) fn new(
        ctx: &'a mut DataChannelHandlerContext,
        data_channels: &'a mut DataChannelRegistry,
        stats: &'a mut RTCStatsAccumulator,
        dtls_role: RTCDtlsRole,
        max_channels: u16,
    ) -> Self {
        DataChannelHandler {
            ctx,
            data_channels,
            stats,
            dtls_role,
            max_channels,
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        "DataChannelHandler"
    }
}

impl<'a> sansio::Protocol<TaggedRTCMessageInternal, TaggedRTCMessageInternal, RTCEventInternal>
    for DataChannelHandler<'a>
{
    type Rout = TaggedRTCMessageInternal;
    type Wout = TaggedRTCMessageInternal;
    type Eout = RTCEventInternal;
    type Error = Error;
    type Time = Instant;

    fn handle_read(&mut self, msg: TaggedRTCMessageInternal) -> Result<()> {
        if let RTCMessageInternal::Dtls(DTLSMessage::Sctp(message)) = msg.message {
            debug!(
                "recv SCTP DataChannelMessage from {:?}",
                msg.transport.peer_addr
            );

            let stream_id = message.stream_id;

            // A negotiated channel still waiting for `SCTPHandshakeComplete` opens on its first
            // message instead. The peer's association can be up before this one is, so its first
            // DATA can arrive in the same read burst as the datagram that completes this side's
            // handshake, ahead of the event. SCTP has already acknowledged that DATA, so dropping
            // it here would lose it for good.
            if let Some(handle) = self.data_channels.handle_of_stream(&stream_id)
                && let Some(waiting) = self.data_channels.get_mut(&handle)
                && waiting.ready_state == RTCDataChannelState::Connecting
            {
                open_connecting(
                    self.ctx,
                    self.stats,
                    handle,
                    waiting,
                    message.association_handle,
                )?;
            }

            // SCTP addresses channels by stream id; everything leaving this handler towards
            // the application is keyed by handle instead.
            if let Some(data_channel_internal) = self.data_channels.get_by_stream_mut(&stream_id) {
                let data_channel = data_channel_internal
                    .data_channel
                    .as_mut()
                    .ok_or(Error::ErrDataChannelNotExisted)?;
                data_channel.handle_read(message)?;
            } else {
                let data_channel_internal = RTCDataChannelInternal::accept(
                    message.association_handle,
                    message.stream_id,
                    message.ppi,
                    &message.payload,
                )?;

                let label = data_channel_internal.label.clone();
                let protocol = data_channel_internal.protocol.clone();
                let handle = self.data_channels.insert(data_channel_internal);

                self.ctx.read_outs.push_back(TaggedRTCMessageInternal {
                    now: msg.now,
                    transport: msg.transport,
                    message: RTCMessageInternal::Dtls(DTLSMessage::DataChannel(
                        ApplicationMessage {
                            data_channel_id: handle,
                            data_channel_event: DataChannelEvent::Open,
                        },
                    )),
                });

                // Track data channel opened
                self.stats.peer_connection.on_data_channel_opened();
                // Initialize data channel stats. Keyed by handle; the W3C
                // `dataChannelIdentifier` it reports is the wire value.
                self.stats
                    .get_or_create_data_channel(handle, &label, &protocol)
                    .on_state_changed(RTCDataChannelState::Open);
                self.stats.set_data_channel_stream_id(handle, stream_id);
            }

            // From here on the channel is addressed by handle, which is what the application
            // and every event it receives use.
            let channel_id = self
                .data_channels
                .handle_of_stream(&stream_id)
                .ok_or(Error::ErrDataChannelNotExisted)?;

            // Get label/protocol before taking mutable borrow for the loop
            let (label, protocol) = {
                let dc = self
                    .data_channels
                    .get(&channel_id)
                    .ok_or(Error::ErrDataChannelNotExisted)?;
                (dc.label.clone(), dc.protocol.clone())
            };

            let data_channel = self
                .data_channels
                .get_mut(&channel_id)
                .ok_or(Error::ErrDataChannelNotExisted)?
                .data_channel
                .as_mut()
                .ok_or(Error::ErrDataChannelNotExisted)?;

            while let Some(data_channel_message) = data_channel.poll_read() {
                let payload_len = data_channel_message.payload.len();
                debug!("recv application message {:?}", msg.transport.peer_addr);

                // Track received message stats
                self.stats
                    .get_or_create_data_channel(channel_id, &label, &protocol)
                    .on_message_received(payload_len);

                // https://tools.ietf.org/html/draft-ietf-rtcweb-data-channel-12#section-6.6
                // When receiving an SCTP user message with one of these [Empty]
                // PPIDs, the receiver MUST ignore the SCTP user message and
                // process it as an empty message.
                let message_data = if matches!(
                    data_channel_message.ppi,
                    PayloadProtocolIdentifier::StringEmpty | PayloadProtocolIdentifier::BinaryEmpty
                ) {
                    Default::default()
                } else {
                    data_channel_message.payload
                };

                self.ctx.read_outs.push_back(TaggedRTCMessageInternal {
                    now: msg.now,
                    transport: msg.transport,
                    message: RTCMessageInternal::Dtls(DTLSMessage::DataChannel(
                        ApplicationMessage {
                            data_channel_id: channel_id,
                            data_channel_event: DataChannelEvent::Message(RTCDataChannelMessage {
                                is_string: matches!(
                                    data_channel_message.ppi,
                                    PayloadProtocolIdentifier::String
                                        | PayloadProtocolIdentifier::StringEmpty
                                ),
                                data: message_data,
                            }),
                        },
                    )),
                });
            }

            while let Some(data_channel_message) = data_channel.poll_write() {
                debug!("send data channel message from handle_read");
                self.ctx.write_outs.push_back(TaggedRTCMessageInternal {
                    now: Instant::now(),
                    transport: TransportContext::default(),
                    message: RTCMessageInternal::Dtls(DTLSMessage::Sctp(data_channel_message)),
                });
            }
        } else {
            // Bypass
            debug!("bypass DataChannel read {:?}", msg.transport.peer_addr);
            self.ctx.read_outs.push_back(msg);
        }
        Ok(())
    }

    fn poll_read(&mut self) -> Option<Self::Rout> {
        self.ctx.read_outs.pop_front()
    }

    fn handle_write(&mut self, msg: TaggedRTCMessageInternal) -> Result<()> {
        if let RTCMessageInternal::Dtls(DTLSMessage::DataChannel(message)) = msg.message {
            debug!("send application message {:?}", msg.transport.peer_addr);

            if let DataChannelEvent::Message(RTCDataChannelMessage { is_string, data }) =
                message.data_channel_event
            {
                let data_len = data.len();
                let channel_id = message.data_channel_id;

                // Get label/protocol before taking mutable borrow
                let dc_internal = self
                    .data_channels
                    .get(&channel_id)
                    .ok_or(Error::ErrDataChannelNotExisted)?;
                let label = dc_internal.label.clone();
                let protocol = dc_internal.protocol.clone();

                let data_channel = self
                    .data_channels
                    .get_mut(&channel_id)
                    .ok_or(Error::ErrDataChannelNotExisted)?
                    .data_channel
                    .as_mut()
                    .ok_or(Error::ErrDataChannelNotExisted)?;

                let data_channel_message =
                    ::datachannel::data_channel::DataChannel::get_data_channel_message(
                        is_string, data,
                    );
                data_channel.handle_write(data_channel_message)?;

                // Track sent message stats
                self.stats
                    .get_or_create_data_channel(channel_id, &label, &protocol)
                    .on_message_sent(data_len);

                while let Some(data_channel_message) = data_channel.poll_write() {
                    debug!("send data channel message from handle_write");
                    self.ctx.write_outs.push_back(TaggedRTCMessageInternal {
                        now: Instant::now(),
                        transport: TransportContext::default(),
                        message: RTCMessageInternal::Dtls(DTLSMessage::Sctp(data_channel_message)),
                    });
                }
            } else {
                warn!(
                    "drop unsupported DATACHANNEL message to {}",
                    msg.transport.peer_addr
                );
            }
        } else {
            // Bypass
            debug!("bypass DataChannel write {:?}", msg.transport.peer_addr);
            self.ctx.write_outs.push_back(msg);
        }
        Ok(())
    }

    fn poll_write(&mut self) -> Option<Self::Wout> {
        for data_channel_internal in self.data_channels.values_mut() {
            if let Some(data_channel) = data_channel_internal.data_channel.as_mut() {
                while let Some(data_channel_message) = data_channel.poll_write() {
                    debug!("send data channel message from poll_write");
                    self.ctx.write_outs.push_back(TaggedRTCMessageInternal {
                        now: Instant::now(),
                        transport: TransportContext::default(),
                        message: RTCMessageInternal::Dtls(DTLSMessage::Sctp(data_channel_message)),
                    });
                }
            }
        }

        self.ctx.write_outs.pop_front()
    }

    fn handle_event(&mut self, evt: RTCEventInternal) -> Result<()> {
        match evt {
            RTCEventInternal::SCTPHandshakeComplete(association_handle) => {
                // The W3C "RTCSctpTransport connected procedure": the association is up, so
                // the DTLS role is resolved and the negotiated stream count is known. This is
                // the first moment a stream id can be chosen correctly, and therefore the
                // moment it is chosen at all (RFC 8832 §6).
                self.data_channels
                    .assign_stream_ids(self.dtls_role, self.max_channels)?;

                for (handle, data_channel_internal) in self.data_channels.iter_mut() {
                    if data_channel_internal.ready_state == RTCDataChannelState::Connecting {
                        open_connecting(
                            self.ctx,
                            self.stats,
                            handle,
                            data_channel_internal,
                            association_handle,
                        )?;
                    }
                }
            }

            RTCEventInternal::SCTPStreamClosed(_association_handle, stream_id) => {
                // The event names the channel by handle, as every application-facing event
                // does; the stream id was only how SCTP referred to it.
                if let Some((channel_id, _dc)) = self.data_channels.remove_by_stream(&stream_id) {
                    // Track data channel closed
                    self.stats.peer_connection.on_data_channel_closed();
                    if let Some(dc_stats) = self.stats.data_channels.get_mut(&channel_id) {
                        dc_stats.on_state_changed(RTCDataChannelState::Closed);
                    }

                    self.ctx
                        .event_outs
                        .push_back(RTCEventInternal::RTCPeerConnectionEvent(
                            RTCPeerConnectionEvent::OnDataChannel(RTCDataChannelEvent::OnClose(
                                channel_id,
                            )),
                        ));
                }
            }
            RTCEventInternal::SCTPBufferReleased(_association_handle, stream_id, n_bytes) => {
                // Pure accounting: SCTP released (acked or abandoned) `n_bytes` of
                // this channel's outgoing buffer. Decrement the synchronous send
                // back-pressure counter; do NOT forward the event further.
                if let Some(dc) = self.data_channels.get_by_stream_mut(&stream_id) {
                    dc.outstanding_bytes = dc.outstanding_bytes.saturating_sub(n_bytes);
                }
            }

            // The SCTP layer knows only stream ids; the application-facing events name the
            // channel by handle, like every other event it receives. This is the layer that
            // owns the mapping, so the translation happens here.
            RTCEventInternal::SCTPBufferedAmountLow(_association_handle, stream_id) => {
                if let Some(channel_id) = self.data_channels.handle_of_stream(&stream_id) {
                    self.ctx
                        .event_outs
                        .push_back(RTCEventInternal::RTCPeerConnectionEvent(
                            RTCPeerConnectionEvent::OnDataChannel(
                                RTCDataChannelEvent::OnBufferedAmountLow(channel_id),
                            ),
                        ));
                }
            }
            RTCEventInternal::SCTPBufferedAmountHigh(_association_handle, stream_id) => {
                if let Some(channel_id) = self.data_channels.handle_of_stream(&stream_id) {
                    self.ctx
                        .event_outs
                        .push_back(RTCEventInternal::RTCPeerConnectionEvent(
                            RTCPeerConnectionEvent::OnDataChannel(
                                RTCDataChannelEvent::OnBufferedAmountHigh(channel_id),
                            ),
                        ));
                }
            }
            _ => {
                self.ctx.event_outs.push_back(evt);
            }
        }
        Ok(())
    }

    fn poll_event(&mut self) -> Option<Self::Eout> {
        self.ctx.event_outs.pop_front()
    }

    fn handle_timeout(&mut self, _now: Instant) -> Result<()> {
        Ok(())
    }

    fn poll_timeout(&mut self) -> Option<Instant> {
        None
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Dial a channel that was waiting for its association, announce it open and queue what the
/// dial has to send. The W3C "RTCSctpTransport connected procedure" for one channel: run for
/// every connecting channel when the handshake completes, and for a negotiated one when the
/// peer's first message on its stream gets here before that.
fn open_connecting(
    ctx: &mut DataChannelHandlerContext,
    stats: &mut RTCStatsAccumulator,
    handle: RTCDataChannelId,
    data_channel_internal: &mut RTCDataChannelInternal,
    association_handle: usize,
) -> Result<()> {
    data_channel_internal.dial(association_handle)?;

    let data_channel = data_channel_internal
        .data_channel
        .as_mut()
        .ok_or(Error::ErrDataChannelNotExisted)?;

    ctx.read_outs.push_back(TaggedRTCMessageInternal {
        now: Instant::now(),
        transport: TransportContext::default(),
        message: RTCMessageInternal::Dtls(DTLSMessage::DataChannel(ApplicationMessage {
            data_channel_id: handle,
            data_channel_event: DataChannelEvent::Open,
        })),
    });

    // Track data channel opened (initiator side)
    stats.peer_connection.on_data_channel_opened();
    stats
        .get_or_create_data_channel(
            handle,
            &data_channel_internal.label,
            &data_channel_internal.protocol,
        )
        .on_state_changed(RTCDataChannelState::Open);

    while let Some(data_channel_message) = data_channel.poll_write() {
        debug!("send data channel message from handle_event");
        ctx.write_outs.push_back(TaggedRTCMessageInternal {
            now: Instant::now(),
            transport: TransportContext::default(),
            message: RTCMessageInternal::Dtls(DTLSMessage::Sctp(data_channel_message)),
        });
    }
    Ok(())
}
