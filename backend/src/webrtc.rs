//! Session WebRTC pour le streaming vidéo H.264 basse latence.
//!
//! Gère l'établissement de la PeerConnection (via le signaling WebSocket), l'encodage
//! des frames JPEG/RGB de la watch-channel en H.264, et l'envoi des paquets RTP.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::media::Sample;

use crate::h264::H264Encoder;
use crate::protocol::ServerMessage;

pub struct WebRtcSession {
    peer_connection: Arc<RTCPeerConnection>,
    #[allow(dead_code)]
    track: Arc<TrackLocalStaticSample>,
    forward_task: Option<tokio::task::JoinHandle<()>>,
}

impl WebRtcSession {
    /// Initialise la PeerConnection et renvoie l'offre SDP locale.
    pub async fn create<F>(
        rx_frames: watch::Receiver<Vec<u8>>,
        on_candidate: F,
    ) -> anyhow::Result<(Self, String)>
    where
        F: Fn(ServerMessage) + Send + Sync + 'static,
    {
        // 1. Initialise le MediaEngine et enregistre les codecs par défaut
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let api = APIBuilder::new().with_media_engine(m).build();

        // 2. Configure les serveurs ICE (STUN publique par défaut pour le POC)
        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }],
            ..Default::default()
        };

        let peer_connection = Arc::new(api.new_peer_connection(config).await?);

        // 3. Crée la piste vidéo locale H.264
        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: "video/h264".to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "appmirror-stream".to_owned(),
        ));

        // Ajoute la piste à la PeerConnection
        let track_dyn = Arc::clone(&track) as Arc<dyn webrtc::track::track_local::TrackLocal + Send + Sync>;
        peer_connection.add_track(track_dyn).await?;

        // 4. Configure les callbacks ICE Candidate
        let on_candidate_arc = Arc::new(on_candidate);
        let on_candidate_clone = Arc::clone(&on_candidate_arc);
        peer_connection.on_ice_candidate(Box::new(move |c: Option<RTCIceCandidate>| {
            let callback = Arc::clone(&on_candidate_clone);
            Box::pin(async move {
                if let Some(candidate) = c {
                    if let Ok(candidate_init) = candidate.to_json() {
                        let msg = ServerMessage::WebrtcIceCandidate {
                            candidate: candidate_init.candidate,
                            sdp_mid: candidate_init.sdp_mid,
                            sdp_mline_index: candidate_init.sdp_mline_index,
                        };
                        callback(msg);
                    }
                }
            })
        }));

        // 5. Crée l'offre SDP locale
        let offer = peer_connection.create_offer(None).await?;
        let offer_sdp = offer.sdp.clone();
        peer_connection.set_local_description(offer).await?;

        // 6. Démarre la tâche de forwarding et encodage des frames
        let track_clone = Arc::clone(&track);
        let forward_task = tokio::spawn(async move {
            if let Err(e) = run_frame_forward_loop(rx_frames, track_clone).await {
                tracing::warn!("Sortie de la boucle WebRTC frame forward : {:?}", e);
            }
        });

        Ok((
            Self {
                peer_connection,
                track,
                forward_task: Some(forward_task),
            },
            offer_sdp,
        ))
    }

    /// Enregistre la réponse SDP (Answer) du navigateur.
    pub async fn set_remote_answer(&self, sdp: &str) -> anyhow::Result<()> {
        let answer = RTCSessionDescription::answer(sdp.to_owned())?;
        self.peer_connection.set_remote_description(answer).await?;
        Ok(())
    }

    /// Enregistre un candidat ICE reçu du navigateur.
    pub async fn add_remote_candidate(
        &self,
        candidate: &str,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    ) -> anyhow::Result<()> {
        let init = RTCIceCandidateInit {
            candidate: candidate.to_owned(),
            sdp_mid,
            sdp_mline_index,
            username_fragment: None,
        };
        self.peer_connection.add_ice_candidate(init).await?;
        Ok(())
    }

    /// Arrête la session WebRTC.
    pub async fn stop(mut self) {
        if let Some(task) = self.forward_task.take() {
            task.abort();
        }
        let _ = self.peer_connection.close().await;
    }
}

/// Boucle de lecture, encodage et envoi des frames.
async fn run_frame_forward_loop(
    mut rx: watch::Receiver<Vec<u8>>,
    track: Arc<TrackLocalStaticSample>,
) -> anyhow::Result<()> {
    let mut encoder: Option<H264Encoder> = None;
    let mut last_dimensions = (0, 0);

    while rx.changed().await.is_ok() {
        let frame_bytes = rx.borrow_and_update().clone();
        if frame_bytes.is_empty() {
            continue;
        }

        // Les frames de la watch-channel sont actuellement encodées en JPEG.
        // Pour les décoder en RGB24 et les ré-encoder en H.264, on utilise la crate `image`.
        // C'est un pipeline transitoire pour le POC avant l'extraction directe D3D11/surfaces brutes.
        let decoded = match image::load_from_memory_with_format(&frame_bytes, image::ImageFormat::Jpeg) {
            Ok(img) => img.to_rgb8(),
            Err(e) => {
                tracing::debug!("Erreur décodage JPEG frame : {:?}", e);
                continue;
            }
        };

        let (w, h) = decoded.dimensions();
        if last_dimensions != (w, h) || encoder.is_none() {
            // Recrée l'encodeur si la résolution de la fenêtre a changé
            encoder = Some(H264Encoder::new(w, h)?);
            last_dimensions = (w, h);
            tracing::info!("Encodeur H.264 configuré pour la résolution : {}x{}", w, h);
        }

        if let Some(ref mut enc) = encoder {
            match enc.encode(decoded.as_raw()) {
                Ok(h264_data) => {
                    if h264_data.is_empty() {
                        continue;
                    }
                    // Encapsule et envoie la frame H.264 via RTP
                    let sample = Sample {
                        data: h264_data.into(),
                        duration: Duration::from_millis(33), // ~30 fps
                        ..Default::default()
                    };
                    if let Err(e) = track.write_sample(&sample).await {
                        tracing::warn!("Échec d'écriture du sample RTP : {:?}", e);
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("Échec d'encodage H.264 : {:?}", e);
                }
            }
        }
    }

    Ok(())
}
