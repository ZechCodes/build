// Dot-grid hero background: ripples (agents finishing work) sweep across the field;
// now and then one dot is flagged NEEDS YOU, then resolved by a ripple.
(function(){
const cv=document.getElementById('cvL');if(!cv)return;const x=cv.getContext('2d');
const G=parseFloat(cv.dataset.gap||28),R=1.1;let W,H,cols,rows,dpr;
function size(){dpr=Math.min(2,devicePixelRatio||1);const b=cv.parentElement.getBoundingClientRect();W=b.width;H=b.height;cv.width=W*dpr;cv.height=H*dpr;cv.style.width=W+'px';cv.style.height=H+'px';x.setTransform(dpr,0,0,dpr,0,0);cols=Math.ceil(W/G)+1;rows=Math.ceil(H/G)+1}
size();addEventListener('resize',size);
const rip=[],rnd=n=>Math.floor(Math.random()*n);let flag=null,nextFlag=1800,nextRip=0,last=0;
function spawn(cx,cy,big){rip.push({x:cx,y:cy,r:0,v:big?.13:.09,w:big?110:70,a:1})}
function draw(t){
 const dt=Math.min(60,t-last);last=t;x.clearRect(0,0,W,H);
 nextRip-=dt;if(nextRip<=0){spawn(rnd(cols)*G,rnd(rows)*G,Math.random()<.3);nextRip=1500+Math.random()*1800}
 for(const q of rip){q.r+=q.v*dt;q.a=Math.max(0,1-q.r/(Math.max(W,H)*.9))}
 for(let i=rip.length-1;i>=0;i--)if(rip[i].a<=0)rip.splice(i,1);
 // vignette: dots fade toward center so copy stays legible
 const cx=W/2,cy=H*.48;
 for(let i=0;i<cols;i++)for(let j=0;j<rows;j++){
  const px=i*G,py=j*G;let e=0;
  for(const q of rip){const d=Math.hypot(px-q.x,py-q.y)-q.r;if(Math.abs(d)<q.w){const k=1-Math.abs(d)/q.w;e=Math.max(e,k*k*q.a)}}
  const dc=Math.hypot((px-cx)/(W*.55),(py-cy)/(H*.55)),vig=Math.min(1,Math.max(.25,dc*dc*1.6));
  const al=(.07+e*.75)*vig,r=R+e*1.6;
  if(e>.55){x.shadowColor='rgba(0,255,136,.8)';x.shadowBlur=8}else x.shadowBlur=0;
  x.beginPath();x.arc(px,py,r,0,7);x.fillStyle='rgba(0,255,136,'+al.toFixed(3)+')';x.fill()}
 x.shadowBlur=0;
 nextFlag-=dt;
 if(!flag&&nextFlag<=0){let i,j;do{i=rnd(cols);j=rnd(rows)}while(Math.hypot((i*G-cx)/(W*.55),(j*G-cy)/(H*.55))<.75||i*G>W-110||i*G<20||j*G<30||j*G>H-30);flag={x:i*G,y:j*G,age:0};nextFlag=5500+Math.random()*3000}
 if(flag){flag.age+=dt;const{x:X,y:Y}=flag,r=10,blink=flag.age%900<650;
  if(blink){x.strokeStyle='#eafff4';x.lineWidth=1.5;
   for(const[dx,dy]of[[-1,-1],[1,-1],[-1,1],[1,1]]){x.beginPath();x.moveTo(X+dx*r,Y+dy*r-dy*5);x.lineTo(X+dx*r,Y+dy*r);x.lineTo(X+dx*r-dx*5,Y+dy*r);x.stroke()}
   x.font='500 10px JetBrains Mono';x.fillStyle='#eafff4';x.textAlign='left';x.fillText('NEEDS YOU',X+r+6,Y+3)}
  x.beginPath();x.arc(X,Y,2.6,0,7);x.fillStyle='#fff';x.fill();
  if(flag.age>5200){spawn(X,Y,true);flag=null}}
 requestAnimationFrame(draw)}
requestAnimationFrame(draw)})();
