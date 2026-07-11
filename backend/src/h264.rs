//! Encodeur vidéo H.264 basé sur la crate `openh264`.
//!
//! Cet encodeur prend des frames RGB brutes issues de la capture native (Windows/Linux)
//! et produit des paquets H.264 (NAL units) pour le flux WebRTC.

use openh264::encoder::{Encoder, EncoderConfig};
use openh264::formats::YUVBuffer;

pub struct H264Encoder {
    encoder: Encoder,
    width: u32,
    height: u32,
}

impl H264Encoder {
    /// Crée un nouvel encodeur H.264 avec les dimensions données.
    pub fn new(width: u32, height: u32) -> anyhow::Result<Self> {
        // openh264 nécessite des dimensions paires. On les arrondit vers le bas si besoin.
        let w = width & !1;
        let h = height & !1;

        let config = EncoderConfig::new(w, h);
        let encoder = Encoder::with_config(config)
            .map_err(|e| anyhow::anyhow!("Échec d'initialisation de l'encodeur openh264: {e:?}"))?;

        Ok(Self {
            encoder,
            width: w,
            height: h,
        })
    }

    /// Encode une frame RGB24 (3 octets par pixel) en H.264.
    /// Renvoie les données binaires H.264 (NAL units) encodées.
    pub fn encode(&mut self, rgb: &[u8]) -> anyhow::Result<Vec<u8>> {
        // S'assure que la taille correspond à width * height * 3
        let expected_size = (self.width * self.height * 3) as usize;
        if rgb.len() < expected_size {
            anyhow::bail!(
                "Taille de frame RGB invalide: attendu {}, obtenu {}",
                expected_size,
                rgb.len()
            );
        }

        // Convertit en YUV420p (le format requis par l'encodeur OpenH264)
        let yuv = YUVBuffer::with_rgb(self.width as usize, self.height as usize, &rgb[..expected_size]);

        // Encode la frame
        let bitstream = self.encoder.encode(&yuv)
            .map_err(|e| anyhow::anyhow!("Erreur lors de l'encodage H.264: {e:?}"))?;

        // Récupère les octets encodés
        let mut out = Vec::new();
        bitstream.write_vec(&mut out);

        Ok(out)
    }

    #[allow(dead_code)]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[allow(dead_code)]
    pub fn height(&self) -> u32 {
        self.height
    }
}
