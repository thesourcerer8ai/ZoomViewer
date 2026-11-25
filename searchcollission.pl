#!/usr/bin/perl -w
use strict;

my %coll=();
my %maxi=(); # Searching for the page with the most sectors found

foreach my $fn(sort <search_results_*.txt>)
{
  print "$fn\n";	
  open IN,"<$fn";
  my $line1=<IN>;
  my $line2=<IN>;
  my $line3=<IN>;
  while(<IN>)
  {
    my @liste=split /\|/,$_;
    #print;
    #print ":Block $liste[2] :Page $liste[3]\n";
    
    if(defined($coll{$liste[2]}{$liste[3]}) && $coll{$liste[2]}{$liste[3]} ne $fn)
    {
      print "Collission with $fn and $coll{$liste[2]}{$liste[3]} in $liste[2] / $liste[3]: $_";
    }
    $coll{$liste[2]}{$liste[3]}=$fn;		   
    $maxi{$fn." ".$liste[2]." ".$liste[3]}++;
  }
  close IN;
}

foreach(sort {$maxi{$b} <=> $maxi{$a}} keys %maxi)
{
  print "$_: $maxi{$_}\n";
}
