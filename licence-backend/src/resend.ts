/**
 * Resend transactional email. One helper per message kind; copy lives
 * here so non-engineers can grep for it.
 */
import type { Env } from "./env";

async function send(
  env: Env,
  to: string,
  subject: string,
  html: string,
): Promise<void> {
  await fetch("https://api.resend.com/emails", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${env.RESEND_API_KEY}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ from: env.RESEND_FROM, to, subject, html }),
  });
}

export async function sendPurchaseReceipt(
  env: Env,
  to: string,
  licenceKey: string,
): Promise<void> {
  const html = `
    <p>Thanks for buying <strong>Strivo Pro</strong>.</p>
    <p>Your licence key:</p>
    <pre style="font-family:monospace;font-size:14px;background:#f4f4f6;padding:12px;border-radius:6px">${licenceKey}</pre>
    <p>Paste it into the <em>I have a key</em> dialog on the Plugins page in StriVo. The key activates one machine — if you change machines, contact support and we'll move it.</p>
    <p>— StriVo</p>`;
  await send(env, to, "Your Strivo Pro licence", html);
}

export async function sendRefundNotice(
  env: Env,
  to: string,
): Promise<void> {
  const html = `
    <p>Your Strivo Pro licence has been refunded and will stop unlocking Pro plugins within 72 hours.</p>
    <p>If this wasn't you, reply to this email and we'll sort it out.</p>
    <p>— StriVo</p>`;
  await send(env, to, "Strivo Pro refund processed", html);
}
