#!/usr/bin/env python3
"""Reproduce the Photoshop curve-interpolation oracle behind the
`matches_photoshop_on_curves_from_its_own_merged_composite` test in
`crates/aurora-core/src/tone_curve.rs` (0.157.0).

Independent oracle: `adjustments/curves_rgb.psd` in the psd-tools fixture
corpus stores Photoshop's own merged composite. For each Curves layer whose
mask is fully on where every other Curves mask is fully off, the backdrop
(inputs only) is psd-tools' composite with the Curves layers hidden, and the
expected output is Photoshop's merged pixel. Per input level the modal output
is kept when at least 90% of at least 3 pixels agree. Candidate interpolation
models are scored; the "PAIRS" lines are the reference pairs embedded in the
Rust test.

Run from the corpus root (needs numpy and psd-tools; 1.17.4 was used; not
part of CI):

    cd corpora/psd/reference/psd-tools-fixtures
    python3 ../../../../scripts/oracles/psd_curves_oracle.py
"""
import collections
import sys
import numpy as np
from psd_tools import PSDImage
np.set_printoptions(linewidth=200)
def natural(x,y,t):
    n=len(x)
    if n==2: return np.interp(t,x,y)
    h=np.diff(x); A=np.zeros((n,n)); r=np.zeros(n); A[0,0]=A[-1,-1]=1
    for i in range(1,n-1):
        A[i,i-1]=h[i-1]; A[i,i]=2*(h[i-1]+h[i]); A[i,i+1]=h[i]
        r[i]=6*((y[i+1]-y[i])/h[i]-(y[i]-y[i-1])/h[i-1])
    M=np.linalg.solve(A,r)
    tt=np.clip(t,x[0],x[-1]); k=np.clip(np.searchsorted(x,tt,side='right')-1,0,n-2)
    a=x[k+1]-tt; b=tt-x[k]; hk=h[k]
    return (M[k]*a**3+M[k+1]*b**3)/(6*hk)+(y[k]/hk-M[k]*hk/6)*a+(y[k+1]/hk-M[k+1]*hk/6)*b
def clamped0(x,y,t):  # zero end slopes
    n=len(x); h=np.diff(x); A=np.zeros((n,n)); r=np.zeros(n)
    A[0,0]=2*h[0];A[0,1]=h[0]; r[0]=6*((y[1]-y[0])/h[0])
    A[-1,-1]=2*h[-1];A[-1,-2]=h[-1]; r[-1]=-6*((y[-1]-y[-2])/h[-1])
    for i in range(1,n-1):
        A[i,i-1]=h[i-1]; A[i,i]=2*(h[i-1]+h[i]); A[i,i+1]=h[i]
        r[i]=6*((y[i+1]-y[i])/h[i]-(y[i]-y[i-1])/h[i-1])
    M=np.linalg.solve(A,r)
    tt=np.clip(t,x[0],x[-1]); k=np.clip(np.searchsorted(x,tt,side='right')-1,0,n-2)
    a=x[k+1]-tt; b=tt-x[k]; hk=h[k]
    return (M[k]*a**3+M[k+1]*b**3)/(6*hk)+(y[k]/hk-M[k]*hk/6)*a+(y[k+1]/hk-M[k+1]*hk/6)*b
def fc(x,y,t):
    n=len(x); h=np.diff(x); d=np.diff(y)/h; m=np.zeros(n); m[0]=d[0]; m[-1]=d[-1]
    for i in range(1,n-1):
        m[i]=0 if d[i-1]*d[i]<=0 else (d[i-1]+d[i])/2
    for i in range(n-1):
        if d[i]==0: m[i]=m[i+1]=0; continue
        a=m[i]/d[i]; b=m[i+1]/d[i]; s=a*a+b*b
        if s>9: tau=3/np.sqrt(s); m[i]=tau*a*d[i]; m[i+1]=tau*b*d[i]
    tt=np.clip(t,x[0],x[-1]); k=np.clip(np.searchsorted(x,tt,side='right')-1,0,n-2)
    hk=h[k]; s=(tt-x[k])/hk
    h00=2*s**3-3*s**2+1;h10=s**3-2*s**2+s;h01=-2*s**3+3*s**2;h11=s**3-s**2
    return h00*y[k]+h10*hk*m[k]+h01*y[k+1]+h11*hk*m[k+1]
def linear(x,y,t): return np.interp(t,x,y)
MODELS={'natural':natural,'clamped-zero-slope':clamped0,'fritsch-carlson':fc,'linear':linear}
from psd_tools import PSDImage
p=PSDImage.open('adjustments/curves_rgb.psd')
merged=np.asarray(p.topil()).astype(int)[...,:3]
back=np.asarray(p.composite(layer_filter=lambda l: type(l).__name__!='Curves' and l.is_visible())).astype(int)[...,:3]
H,W=200,200
curves={l.name:l for l in p.descendants() if type(l).__name__=='Curves'}
def maskarr(l):
    m=np.full((H,W),1.0); mk=l.mask; m[:]=mk.background_color/255.0
    mi=np.asarray(mk.topil()).astype(np.float64)/255.0; L,T,R,B=mk.bbox
    for yy in range(mi.shape[0]):
        Y=T+yy
        if 0<=Y<H:
            x0=max(L,0); x1=min(L+mi.shape[1],W)
            if x1>x0: m[Y,x0:x1]=mi[yy,x0-L:x1-L]
    return m
masks={n:maskarr(l) for n,l in curves.items()}
def solo(n):
    o=masks[n]==1.0
    for k in masks:
        if k!=n: o&=masks[k]==0.0
    return o
def pts(n,cid):
    d={e.channel_id:e.points for e in curves[n].extra}[cid]
    return [(q[1],q[0]) for q in d]
t=np.arange(256.0)
def lutf(fn,pp):
    x=np.array([a for a,b in pp],float); y=np.array([b for a,b in pp],float); return np.clip(fn(x,y,t),0,255)
for name,ch,cid,comp in [('Curves 3',0,1,False),('Curves 1',1,2,True),('Curves 1',2,3,True)]:
    o=solo(name); bi=back[o][:,ch]; mo=merged[o][:,ch]
    groups=collections.defaultdict(list)
    for a,b in zip(bi,mo): groups[a].append(b)
    pairs=[]
    for a in sorted(groups):
        c=collections.Counter(groups[a]); v,cnt=c.most_common(1)[0]
        if cnt/len(groups[a])>=0.9 and len(groups[a])>=3: pairs.append((int(a),int(v)))
    print(name,ch,'channel pts',pts(name,cid),'comp',pts(name,0) if comp else None,'levels',len(groups),'kept',len(pairs))
    for mname,fn in [('natural',natural),('fc',fc),('linear',linear),('clamped0',clamped0)]:
        L=lutf(fn,pts(name,cid))
        def f(a):
            v=L[a]
            if comp: v=lutf(fn,pts(name,0))[int(np.round(v))]
            return v
        e=[abs(np.round(f(a))-b) for a,b in pairs]
        print('   ',mname,'max',max(e),'n>1',sum(1 for q in e if q>1),'n>0',sum(1 for q in e if q>0))
    print('   PAIRS', ''.join(f'({a},{b}),' for a,b in pairs))
