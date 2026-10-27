/** Quiet language marks, exported once for both native and web module cards. */
import {readFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
const sharp=createRequire(import.meta.url)('sharp');
const scenes={python:[null,'#b3c9a4','py'],'c-cpp':[null,'#9ab9cf','C++'],lisp:[null,'#b7a6ca','λ'],html:['html5','#cba890'],css:['css','#9cb9d5'],markdown:['markdown','#b4c8bb'],restructuredtext:[null,'#c9c0a3','.rst'],'shell-make':[null,'#a4c6b8','sh'],assembler:[null,'#b9bdad','0x']};
const [id,destination]=process.argv.slice(2);
scenes.docker=[null,'#9abed5','▤'];
if(!Object.hasOwn(scenes,id)||!destination)throw new Error('Usage: node module-covers.mjs <module id> <output PNG>');
const [source,color,glyph]=scenes[id];
let mark;
if(source){
  const svg=await readFile(new URL(`../assets/module-marks/${source}.svg`,import.meta.url),'utf8');
  mark=`<g transform="scale(13.333333)" fill="${color}">${svg.replace(/^[\s\S]*?<svg[^>]*>/,'').replace(/<\/svg>[\s\S]*$/,'').replace(/<title>[\s\S]*?<\/title>/g,'')}</g>`;
}else{
  const fontSize=id==='restructuredtext'?145:id==='c-cpp'?150:['python','shell-make'].includes(id)?220:260;
  mark=`<text x="160" y="215" text-anchor="middle" font-family="${id==='lisp'?'Georgia':'Arial'},serif" font-weight="${id==='lisp'?'400':'600'}" font-size="${fontSize}" fill="${color}" letter-spacing="-12">${glyph}</text>`;
}
const svg=`<svg width="640" height="360" viewBox="0 0 640 360" xmlns="http://www.w3.org/2000/svg"><defs><linearGradient id="surface" x2="1" y2="1"><stop stop-color="#1b211e"/><stop offset="1" stop-color="#131919"/></linearGradient><linearGradient id="fade"><stop stop-color="#141a17" stop-opacity=".8"/><stop offset="1" stop-color="#141a17" stop-opacity="0"/></linearGradient></defs><rect width="640" height="360" fill="url(#surface)"/><g transform="translate(304 12)" opacity=".23">${mark}</g><rect width="640" height="360" fill="url(#fade)"/><path d="M0 .5H640" stroke="${color}" stroke-opacity=".18"/></svg>`;
await sharp(Buffer.from(svg),{density:144}).resize(640,360).png().toFile(destination);
