"use client";

import { useState } from "react";

const GITHUB = "https://github.com/revoydotdev/strivo";

const Icon = ({ name, size = 20 }: { name: string; size?: number }) => {
  const paths: Record<string, React.ReactNode> = {
    arrow: <><path d="M5 12h14"/><path d="m13 6 6 6-6 6"/></>,
    check: <path d="m5 12 4 4L19 6"/>,
    github: <><path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3.3-.4 6.8-1.6 6.8-7A5.4 5.4 0 0 0 19.4 4 5 5 0 0 0 19.2.5S18.1.1 15 1.8a13.4 13.4 0 0 0-7 0C4.9.1 3.8.5 3.8.5A5 5 0 0 0 3.6 4a5.4 5.4 0 0 0-1.4 3.7c0 5.4 3.5 6.5 6.8 7A4.8 4.8 0 0 0 8 18v4"/><path d="M8 19c-3 .9-3-1.5-4-2"/></>,
    menu: <><path d="M4 7h16"/><path d="M4 12h16"/><path d="M4 17h16"/></>,
    close: <><path d="m6 6 12 12"/><path d="m18 6-12 12"/></>,
    server: <><rect width="18" height="8" x="3" y="3" rx="2"/><rect width="18" height="8" x="3" y="13" rx="2"/><path d="M7 7h.01M7 17h.01"/></>,
    bell: <><path d="M18 8A6 6 0 0 0 6 8c0 7-3 7-3 9h18c0-2-3-2-3-9"/><path d="M10 21h4"/></>,
    archive: <><rect width="20" height="5" x="2" y="3" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8M10 12h4"/></>,
    lock: <><rect width="18" height="11" x="3" y="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></>,
    bolt: <path d="m13 2-9 12h8l-1 8 9-12h-8z"/>,
    layers: <><path d="m12 2 9 5-9 5-9-5 9-5Z"/><path d="m3 12 9 5 9-5"/><path d="m3 17 9 5 9-5"/></>,
    scissors: <><circle cx="6" cy="7" r="3"/><circle cx="6" cy="17" r="3"/><path d="m8.7 8.4 11.3 6.1M8.7 15.6 20 9.5"/></>,
    text: <><path d="M4 7V4h16v3M9 20h6M12 4v16"/></>,
    shield: <path d="M20 13c0 5-3.5 7.5-8 9-4.5-1.5-8-4-8-9V5l8-3 8 3v8Z"/>,
    image: <><rect width="20" height="18" x="2" y="3" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><path d="m21 15-5-5L5 21"/></>,
    external: <><path d="M15 3h6v6M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/></>,
  };
  return <svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">{paths[name]}</svg>;
};

function Mark({ compact = false }: { compact?: boolean }) {
  return <a className="mark" href="#top" aria-label="StriVo home"><span className="mark-glyph"><i/><i/><i/></span>{!compact && <span>stri<span>vo</span></span>}</a>;
}

function SignalWindow() {
  return (
    <div className="signal-window reveal" aria-label="StriVo library interface preview">
      <div className="window-bar"><div className="traffic"><i/><i/><i/></div><span>strivo.local / library</span><div className="live-pill"><i/> 3 live</div></div>
      <div className="app-shell">
        <aside className="app-rail"><Mark compact/><nav><b/><b/><b/><b/></nav><span/></aside>
        <div className="app-main">
          <div className="app-head"><div><small>GOOD EVENING</small><strong>Your signal is clear.</strong></div><div className="round"/></div>
          <div className="metric-row">
            <div><span>CHANNELS</span><b>18</b><em>+2 this week</em></div>
            <div><span>CAPTURED</span><b>426h</b><em>12.4 TB</em></div>
            <div><span>UP NEXT</span><b>04</b><em>next in 18m</em></div>
          </div>
          <div className="app-section-title"><b>Live now</b><span>View all →</span></div>
          <div className="stream-row">
            <div className="stream-card stream-one"><span className="card-live">● REC 01:42:16</span><div className="wave">{Array.from({length: 28}).map((_,i)=><i key={i}/>)}</div><strong>Analog Hours</strong><small>YouTube · 1080p</small></div>
            <div className="stream-card stream-two"><span className="card-live">● LIVE</span><div className="orbit"><i/><i/><i/></div><strong>Night Shift Build</strong><small>Twitch · 1440p</small></div>
          </div>
          <div className="capture-line"><span className="avatar">N</span><div><b>Night Shift Build</b><small>Recording · 12.8 Mbps</small></div><div className="level"/><time>01:42:16</time></div>
        </div>
      </div>
    </div>
  );
}

const coreFeatures = [
  {icon:"bell", kicker:"WATCH", title:"Follow the signal", text:"Track the channels you care about across Twitch, YouTube, and Patreon from one calm command center."},
  {icon:"bolt", kicker:"CAPTURE", title:"Ready before they go live", text:"StriVo monitors schedules and live states, then starts a resilient recording the moment a broadcast begins."},
  {icon:"archive", kicker:"KEEP", title:"A library that is yours", text:"Search, play, organize, and retain recordings on storage you control—without depending on expiring VODs."},
  {icon:"lock", kicker:"CONTROL", title:"Private by architecture", text:"Self-hosted, local-first, and transparent. Your credentials, media, and viewing history stay on your machine."},
  {icon:"server", kicker:"RELIABLE", title:"Built like infrastructure", text:"A compact Rust service, durable SQLite state, live server events, and explicit recovery paths keep the recorder steady."},
  {icon:"layers", kicker:"UNIFIED", title:"One view, every platform", text:"Consistent discovery and recording workflows make fragmented streaming ecosystems feel like one library."},
];

const dag = [
  ["01","Ingest","Register media"],
  ["02","Probe","Read the signal"],
  ["03","Transcribe","Speech to text"],
  ["04","Understand","Scenes + cues"],
  ["05","Select","Find highlights"],
  ["06","Craft","Clip + reframe"],
  ["07","Finish","Audio + captions"],
  ["08","Brand","Thumbs + safety"],
  ["09","Review","Human approval"],
  ["10","Publish","Draft outputs"],
];

export default function Home() {
  const [menu, setMenu] = useState(false);
  const [copied, setCopied] = useState(false);
  const copyInstall = async () => {
    await navigator.clipboard.writeText("yay -S strivo");
    setCopied(true); setTimeout(()=>setCopied(false), 1600);
  };
  return (
    <main id="top">
      <header className="nav"><Mark/><nav className={menu ? "open" : ""}>
        <a href="#product" onClick={()=>setMenu(false)}>Product</a><a href="#workflow" onClick={()=>setMenu(false)}>How it works</a><a href="#pro" onClick={()=>setMenu(false)}>StriVo Pro</a><a href="#technical" onClick={()=>setMenu(false)}>Architecture</a>
        <a className="nav-github" href={GITHUB} target="_blank" rel="noreferrer"><Icon name="github" size={17}/> GitHub</a>
      </nav><a className="nav-cta" href="#install">Get StriVo <Icon name="arrow" size={16}/></a>
      <button className="menu" onClick={()=>setMenu(!menu)} aria-label="Toggle navigation"><Icon name={menu?"close":"menu"}/></button>
      </header>

      <section className="hero">
        <div className="hero-grid"/>
        <div className="eyebrow"><i/> SELF-HOSTED LIVE-STREAM PVR <span>ALPHA 0.5</span></div>
        <h1>Never miss the moment.<br/><span>Own the archive.</span></h1>
        <p className="hero-copy">StriVo watches your favorite channels, records broadcasts as they happen, and turns fleeting live streams into a library you control.</p>
        <div className="hero-actions"><a className="button primary" href="#install">Start self-hosting <Icon name="arrow"/></a><a className="button secondary" href={GITHUB} target="_blank" rel="noreferrer"><Icon name="github"/> View source</a></div>
        <div className="platforms"><span>BUILT FOR THE STREAMS YOU FOLLOW</span><div><b className="twitch">Twitch</b><b className="youtube">YouTube</b><b className="patreon">Patreon</b></div></div>
        <SignalWindow/>
      </section>

      <section className="manifesto section">
        <p className="section-kicker">THE LIVE WEB NEEDS A MEMORY</p>
        <h2>Streams disappear.<br/>Yours don&apos;t have to.</h2>
        <div className="manifesto-copy"><p>Great broadcasts are still treated like temporary events: split across platforms, hidden behind changing interfaces, and erased on someone else&apos;s schedule.</p><p>StriVo gives live media the same dependable automation that self-hosters expect from the rest of their library. Follow once. Let it watch. Keep what matters.</p></div>
      </section>

      <section id="product" className="features section">
        <div className="section-heading reveal"><div><p className="section-kicker">STRIVO CORE</p><h2>Your personal broadcast archive.</h2></div><p>Simple enough to disappear into the background. Powerful enough to become the source of truth for every stream you care about.</p></div>
        <div className="feature-grid">{coreFeatures.map((f,i)=><article className={`feature reveal feature-${i+1}`} key={f.title}><div className="feature-icon"><Icon name={f.icon}/></div><small>{f.kicker}</small><h3>{f.title}</h3><p>{f.text}</p>{i===1&&<div className="pulse-track"><i/><span>Monitoring 18 channels</span></div>}{i===2&&<div className="shelf"><i/><i/><i/><i/></div>}</article>)}</div>
      </section>

      <section id="workflow" className="workflow section">
        <div className="section-heading reveal"><div><p className="section-kicker">SET IT. KEEP IT.</p><h2>From live signal to lasting library.</h2></div><p>No browser tabs left running. No screen recorder rituals. StriVo takes care of the whole capture loop.</p></div>
        <div className="steps">
          {[["01","ADD","Follow a channel","Paste a channel URL once. StriVo resolves the source and starts watching."],["02","WATCH","Let StriVo monitor","Schedules and live-state checks happen quietly in the background."],["03","CAPTURE","Wake up to the recording","The completed stream lands in your searchable, playable local library."]].map((s,i)=><article className="step reveal" key={s[0]}><span className="step-number">{s[0]}</span><div className={`step-visual visual-${i}`}><i/><i/><i/><b>{i===0?"CHANNEL ADDED":i===1?"SIGNAL FOUND":"CAPTURE SAFE"}</b></div><small>{s[1]}</small><h3>{s[2]}</h3><p>{s[3]}</p></article>)}
        </div>
      </section>

      <section id="pro" className="pro">
        <div className="pro-noise"/>
        <div className="section pro-inner">
          <div className="pro-top reveal"><div><div className="pro-badge"><span>STRIVO</span> PRO</div><h2>One recording.<br/><em>An entire content system.</em></h2></div><p>Creator Edition turns long-form broadcasts into a structured production pipeline—locally, durably, and with you at the approval gate.</p></div>
          <div className="dag-shell reveal">
            <div className="dag-head"><div><i/> PIPELINE / <b>NIGHT_SHIFT_042</b></div><span>RUNNING · 7 OF 10</span></div>
            <div className="dag-track">{dag.map((d,i)=><div className={`dag-node ${i<6?"done":i===6?"active":""}`} key={d[0]}><span>{i<6?<Icon name="check" size={13}/>:d[0]}</span><div><b>{d[1]}</b><small>{d[2]}</small></div>{i<dag.length-1&&<i className="connector"/>}</div>)}</div>
          </div>
          <div className="pro-grid">
            <article className="pro-card reveal"><Icon name="text"/><small>UNDERSTAND</small><h3>Search what was said.</h3><p>Transcription, scene boundaries, cue detection, and structural analysis turn hours of footage into navigable material.</p><div className="transcript"><span>01:42:08</span><p>...and this is where the entire build finally clicks into place.</p><b>92% moment score</b></div></article>
            <article className="pro-card reveal"><Icon name="scissors"/><small>REPURPOSE</small><h3>Find the moments worth sharing.</h3><p>Score highlights, extract clips, reframe for target formats, and finish audio without rebuilding your workflow for every episode.</p><div className="timeline"><div/><span/><span/><span/><b>12 clips</b></div></article>
            <article className="pro-card reveal"><Icon name="image"/><small>PACKAGE</small><h3>Ready for human judgment.</h3><p>Generate captions and thumbnail candidates, run brand-safety checks, and prepare publish drafts—then approve before anything leaves your system.</p><div className="approval"><i><Icon name="shield" size={16}/></i><span><b>Review gate</b><small>Nothing publishes without you.</small></span><em>READY</em></div></article>
          </div>
          <div className="pro-principle reveal"><span>THE PRO PRINCIPLE</span><p>Automation should remove the repetitive work,<br/><b>not remove the creator.</b></p><div><i>Local media</i><i>Durable runs</i><i>Manual approval</i><i>Recoverable stages</i></div></div>
        </div>
      </section>

      <section id="technical" className="technical section">
        <div className="section-heading reveal"><div><p className="section-kicker">BUILT TO STAY ON</p><h2>Quietly serious infrastructure.</h2></div><p>StriVo is a recorder first. Its architecture favors predictable state, durable files, and visible failure over magic.</p></div>
        <div className="architecture reveal">
          <div className="arch-copy"><h3>Small footprint.<br/>Clear boundaries.</h3><p>A Rust daemon coordinates source adapters, recording jobs, metadata, and a responsive web interface. SQLite carries durable state. Server-sent events keep the UI current without a heavyweight cloud stack.</p><div className="tech-tags"><span>Rust</span><span>Axum</span><span>SQLite</span><span>FFmpeg</span><span>SSE</span><span>React</span></div></div>
          <div className="arch-diagram"><div className="source-stack"><i>TW</i><i>YT</i><i>PA</i></div><span className="flow-line"/><div className="core-node"><Mark compact/><b>STRIVO CORE</b><small>MONITOR · RECORD · INDEX</small></div><span className="flow-line"/><div className="output-stack"><i><Icon name="archive" size={17}/> Library</i><i><Icon name="server" size={17}/> Storage</i><i><Icon name="bolt" size={17}/> Events</i></div></div>
        </div>
        <div className="values">
          {[["OPEN SOURCE","Inspect it, change it, or help shape what comes next."],["SELF-HOSTED","Run on your hardware and choose exactly where media lives."],["RECOVERABLE","Explicit state and restart-safe workflows make failures actionable."],["COMMUNITY-BUILT","A young project with its roadmap—and its rough edges—in the open."]].map(v=><article className="reveal" key={v[0]}><small>{v[0]}</small><p>{v[1]}</p></article>)}
        </div>
      </section>

      <section className="audience section">
        <p className="section-kicker reveal">MADE FOR PEOPLE WHO CARE WHAT LASTS</p>
        <div className="audience-grid">
          <h2 className="reveal">Your streams.<br/>Your reasons.</h2>
          <div>{[["THE SUPERFAN","Keep rare broadcasts, long-running series, and community moments together."],["THE ARCHIVIST","Build a deliberate, searchable record instead of trusting a platform’s retention window."],["THE CREATOR","Turn every live session into source material for clips, captions, and future releases."],["THE RESEARCHER","Preserve time-based media and metadata in a system you can inspect and control."]].map((a,i)=><article className="reveal" key={a[0]}><span>0{i+1}</span><div><small>{a[0]}</small><p>{a[1]}</p></div></article>)}</div>
        </div>
      </section>

      <section id="install" className="install section">
        <div className="install-panel reveal">
          <div className="install-copy"><p className="section-kicker">READY WHEN THE NEXT STREAM STARTS</p><h2>Take ownership<br/>of the live web.</h2><p>StriVo is in active alpha development. Install it, explore it, and help build the live-stream library the open web has been missing.</p><div className="hero-actions"><a className="button primary" href={GITHUB} target="_blank" rel="noreferrer">View on GitHub <Icon name="external" size={17}/></a><a className="button secondary" href={`${GITHUB}#quick-start`} target="_blank" rel="noreferrer">Read quick start</a></div></div>
          <div className="terminal"><div className="terminal-head"><span><i/><i/><i/></span><b>INSTALL / ARCH LINUX</b></div><div className="terminal-body"><p><span>$</span> yay -S strivo</p><button onClick={copyInstall}>{copied?"COPIED":"COPY"}</button><div/><p className="muted"><span>›</span> strivo init</p><p className="success"><span>✓</span> configuration ready</p><p className="success"><span>✓</span> web interface on :8484</p></div><footer><i/> Linux primary <span/> macOS supported <span/> Source builds</footer></div>
        </div>
      </section>

      <section className="faq section">
        <div><p className="section-kicker">THE SHORT ANSWERS</p><h2>Good to know.</h2></div>
        <div>{[
          ["Is StriVo a hosted service?","No. StriVo runs on hardware you control and stores recordings where you choose."],
          ["Which platforms are supported?","The current adapters target Twitch, YouTube, and Patreon. Platform behavior can change, so support evolves with their APIs and delivery systems."],
          ["What makes Pro different?","StriVo Pro adds the post-capture Creator pipeline: transcription, content analysis, clipping, finishing, packaging, review, and publish drafts."],
          ["Is it production-ready?","StriVo is currently alpha software. The core is actively tested and hardened, but expect change and read release notes before upgrading."],
        ].map(f=><details key={f[0]}><summary>{f[0]}<span>+</span></summary><p>{f[1]}</p></details>)}</div>
      </section>

      <footer className="footer section"><div><Mark/><p>Monitor. Capture. Keep.</p></div><div><b>EXPLORE</b><a href="#product">Product</a><a href="#pro">StriVo Pro</a><a href="#technical">Architecture</a></div><div><b>PROJECT</b><a href={GITHUB}>GitHub</a><a href={`${GITHUB}/blob/main/ROADMAP.md`}>Roadmap</a><a href={`${GITHUB}/issues`}>Issues</a></div><div className="footer-note"><span>OPEN SOURCE · SELF-HOSTED</span><p>Built for the moments<br/>that deserve to last.</p></div></footer>
      <div className="legal"><span>© 2026 StriVo contributors</span><span>Alpha software · Built in the open</span></div>
    </main>
  );
}
