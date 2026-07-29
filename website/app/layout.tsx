import type { Metadata, Viewport } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "StriVo — Your live streams, on your terms",
  description:
    "A self-hosted live-stream PVR for Twitch, YouTube, and Patreon. Follow channels, capture broadcasts, and build a library you control.",
  keywords: ["live stream recorder", "self-hosted", "PVR", "Twitch", "YouTube", "creator tools"],
  openGraph: {
    title: "StriVo — Your live streams, on your terms",
    description: "Monitor. Capture. Keep. A self-hosted PVR for live streams.",
    type: "website",
  },
  robots: { index: true, follow: true },
};

export const viewport: Viewport = {
  themeColor: "#080b0f",
  colorScheme: "dark",
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
