#!/usr/bin/perl -w

foreach(1 .. 350)
{
   my $fn=sprintf("/home/philipp/.cache/zoomviewer/mount/node_40/blocks/block_0000/page_%02d.bin",$_);
   open IN,"<$fn";
   my $pagelen=55296;
   my $data="";
   read IN,$data,$pagelen;
   foreach my $n(0 .. $pagelen/4-1)
   {
     my $pos=$n*4;
     my $val=substr($data,$pos,4);
     my $N=0xffffffff-unpack("N",$val);
     my $V=0xffffffff-unpack("V",$val);
     print "$_ $n : $N                            $V\n";
   }
   close IN;
}
