#!/usr/bin/perl -w
use strict;

sub bin2hex($)
{
  my $orig=$_[0];
  my $value="";
  return "" if(!defined($orig) || $orig eq "");
  foreach(0 .. length($orig)-1)
  {
    $value.=sprintf("%02X",unpack("C",substr($orig,$_,1)));
  }
  return $value;
}


my %data=();
open CASE,"<download.case";
while(<CASE>)
{
  if(m/"DATA" StartAddress="(\d+)"/)
  {
    $data{$1}=1;	  
  }
}
close CASE;

open IN,"<search_results_0x1.txt";
my $line1=<IN>;
my $line2=<IN>;
my $line3=<IN>;
while(<IN>)
{
  my @liste=split /\|/,$_;
  my $block=$liste[2]; $block=~s/^\s*//; $block=~s/\s*$//;
  my $page=$liste[3]; $page=~s/^\s*//; $page=~s/\s*$//;
  my $offset=$liste[4]; $offset=~s/^\s*//; $offset=~s/\s*$//;
  next unless(defined($data{$offset}));
  my $dumpfn="/home/philipp/.cache/zoomviewer/mount/node_35/blocks/block_".sprintf("%04d",$block)."/page_".sprintf("%02d",$page).".bin";
  #print $dumpfn."\n";
  open DUMP,"<$dumpfn";
  binmode DUMP;
  seek DUMP,$offset+1024,1;
  my $content="";
  read DUMP,$content,8;
  print bin2hex($content)." ".$_;
  close DUMP;
}
close IN;
