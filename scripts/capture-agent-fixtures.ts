// Capture the webview's public MCP read contract, including errors and coercion.
import {writeFileSync} from 'node:fs';
import {READ_TOOLS} from '../src/lib/agent.ts';
import type {Hop,Trace} from '../src/lib/trace.ts';
function hop(ttl:number,addr:string,rtts:(number|null)[]):Hop{return {ttl,addr,name:null,times:rtts.map((_,i)=>1000+i),rtts};}
function trace(sessionId:string,host:string,hops:Hop[]):Trace{return {id:0,runId:0,loaded:true,paused:false,config:{sessionId,host,protocol:'icmp',port:0,intervalMs:1000,alertLatencyMs:0,alertLossPct:0},addr:hops.at(-1)!.addr!,hops,error:null,alerting:false,events:[{t:1001,kind:'hop',ttl:2,from:'10.0.0.2',to:'10.0.0.9'},{t:1003,kind:'alert',text:'Last 10 probes: 50.0 ms average, 0% loss'}],pathLength:hops.length,target:{...hops.at(-1)!,name:host},lengths:[],pathEnd:hops.length,routeNotified:0,diskStart:0};}
const home=trace('aaa','router.lan',[hop(1,'192.168.1.1',[1,1,1,1])]);
const dns=trace('bbb','dns.example',[hop(1,'192.168.1.1',[1,1,1,1]),hop(2,'10.0.0.9',[10,null,30,20])]);
const cases:unknown[]=[];
for (const traces of [[],[home],[home,dns],[dns,{...dns,config:{...dns.config,sessionId:'ccc'}}]]) {
 for(const tool of Object.keys(READ_TOOLS)) for (const args of [{},{target:'bbb'},{target:' DNS.example ',minutes:0},{target:'10.0.0.9',slices:2},{target:'nowhere'},{target:'bbb',hop:0},{target:'bbb',hop:1,slices:2},{target:'bbb',hop:5},{target:'bbb',minutes:0.01,limit:1,slices:2.7},{target:'bbb',minutes:-1,slices:'2',limit:0},{target:'bbb',minutes:null,hop:null}]) {
  try {cases.push({traces,tool,args,expected:await READ_TOOLS[tool](traces,args)});} catch(error){cases.push({traces,tool,args,error:(error as Error).message});}
 }
}
writeFileSync(new URL('../src-tauri/fixtures/legacy-agent.json',import.meta.url),JSON.stringify(cases,null,2)+'\n');
console.log(`Captured ${cases.length} MCP cases.`);
