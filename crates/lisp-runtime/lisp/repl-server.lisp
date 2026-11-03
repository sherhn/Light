;;; Light REPL server.
;;;
;;; Reads one request per line from stdin:      KIND <TAB> ID <TAB> ESCAPED-PAYLOAD
;;; Writes protocol lines to stdout:            <SOH> KIND <TAB> ID <TAB> ESCAPED-PAYLOAD
;;; Everything else the user's code prints is passed through as plain output.
;;; Escaping: backslash, newline, carriage return and tab are written as \\ \n \r \t.

(defpackage :light-repl
  (:use :cl)
  (:export #:main))

(in-package :light-repl)

#+sbcl
(progn
  (ignore-errors (setf sb-impl::*default-external-format* :utf-8))
  (ignore-errors (setf sb-alien::*default-c-string-external-format* :utf-8))
  (ignore-errors (require :sb-introspect))
  (ignore-errors (sb-ext:disable-debugger)))

(defvar *out* *standard-output*)
(defvar *in* *standard-input*)

(defun escape (s)
  (with-output-to-string (o)
    (loop for c across s
          do (case c
               (#\Newline (write-string "\\n" o))
               (#\Return (write-string "\\r" o))
               (#\Tab (write-string "\\t" o))
               (#\\ (write-string "\\\\" o))
               (t (write-char c o))))))

(defun unescape (s)
  (with-output-to-string (o)
    (let ((i 0) (n (length s)))
      (loop while (< i n)
            do (let ((c (char s i)))
                 (if (and (char= c #\\) (< (1+ i) n))
                     (let ((d (char s (1+ i))))
                       (case d
                         (#\n (write-char #\Newline o))
                         (#\r (write-char #\Return o))
                         (#\t (write-char #\Tab o))
                         (t (write-char d o)))
                       (incf i 2))
                     (progn (write-char c o) (incf i))))))))

(defun emit (kind id payload)
  (fresh-line *out*)
  (format *out* "~C~A~C~A~C~A~%" (code-char 1) kind #\Tab id #\Tab (escape payload))
  (finish-output *out*))

(defun split-request (line)
  (let* ((t1 (position #\Tab line))
         (t2 (and t1 (position #\Tab line :start (1+ t1)))))
    (if (and t1 t2)
        (values (subseq line 0 t1) (subseq line (1+ t1) t2) (unescape (subseq line (1+ t2))))
        (values line "0" ""))))

(defun backtrace-string ()
  #+sbcl
  (with-output-to-string (s)
    (ignore-errors (sb-debug:print-backtrace :stream s :count 25)))
  #-sbcl "")

(defun safe-print (value)
  (handler-case
      (let ((*print-readably* nil) (*print-circle* t))
        (prin1-to-string value))
    (error () "#<unprintable object>")))

(defun do-eval (id code)
  (let ((values nil) (trace nil))
    (handler-case
        (progn
          (handler-bind ((serious-condition
                           (lambda (c)
                             (declare (ignore c))
                             (setf trace (backtrace-string)))))
            (with-input-from-string (in code)
              (let ((eof (list nil)))
                (loop for form = (read in nil eof)
                      until (eq form eof)
                      do (setf values (multiple-value-list (eval form)))))))
          (emit "value" id (format nil "~{~A~^~%~}" (mapcar #'safe-print values))))
      (serious-condition (e)
        (emit "error" id (handler-case (format nil "~A" e)
                           (error () (format nil "~S" (type-of e)))))
        (when (and trace (plusp (length trace)))
          (emit "trace" id trace))))
    (emit "done" id (package-name *package*))))

;;; ------------------------------------------------------------ the debugger
;;;
;;; An evaluation of the user's (`eval-debug`) that signals an error stops where it was: the IDE is told the condition,
;;; its restarts and the frames of the stack (`debugger`), and the server answers requests until one of the restarts
;;; is chosen (`restart`), which goes on from there. `locals` gives the variables of a frame. Every request of the IDE
;;; still works in the meantime; an evaluation that fails in it opens a debugger of its own, one level deeper.

(defun one-line (s)
  (substitute #\Space #\Tab (substitute #\Space #\Return (substitute #\Space #\Newline s))))

(defun safe-princ (x)
  (handler-case (let ((*print-length* 20) (*print-level* 4)) (princ-to-string x))
    (error () (format nil "#<~S>" (type-of x)))))

(defun short-print (value)
  (handler-case
      (let ((*print-readably* nil) (*print-circle* t) (*print-length* 20) (*print-level* 3))
        (let ((s (prin1-to-string value))) (if (> (length s) 300) (concatenate 'string (subseq s 0 300) "...") s)))
    (error () "#<unprintable object>")))

#+sbcl
(defun frame-name (frame)
  (ignore-errors (sb-di:debug-fun-name (sb-di:frame-debug-fun frame))))

#+sbcl
(defun debugger-frames ()
  "The frames of the stack from the one that signalled, without those of the debugger itself."
  (let ((frames (loop for f = (sb-di:top-frame) then (sb-di:frame-down f)
                      for i from 0 below 200
                      while f collect f)))
    (let ((last-signal (position-if (lambda (f)
                                      (member (frame-name f) '(error cerror signal warn sb-kernel::%signal invoke-debugger sb-kernel:internal-error)
                                              :test #'equal))
                                    frames :end (min 40 (length frames)) :from-end t)))
      (if last-signal
          (nthcdr (1+ last-signal) frames)
          (member-if-not (lambda (f) (let ((n (frame-name f)))
                                       (or (and (symbolp n) (eq (symbol-package n) (find-package :light-repl)))
                                           (and (consp n) (search "LIGHT-REPL" (prin1-to-string n))))))
                         frames)))))

#+sbcl
(defun evaluation-frames (frames)
  "FRAMES down to the evaluation of the user's form: the server's own frames, and EVAL's right above them, go."
  (let* ((own (position-if (lambda (f) (eq (frame-name f) 'do-eval-debug)) frames))
         (end (or own (length frames))))
    (loop while (and (> end 0) (member (frame-name (nth (1- end) frames)) '(eval sb-int:simple-eval-in-lexenv sb-impl::%simple-eval)))
          do (decf end))
    (subseq frames 0 end)))

#+sbcl
(defun frame-text (frame)
  (or (ignore-errors
       (with-output-to-string (s)
         (let ((*print-length* 8) (*print-level* 3))
           (sb-debug::print-frame-call frame s))))
      (short-print (frame-name frame))))

#+sbcl
(defun frame-locals (frame)
  "name <TAB> value lines of the variables valid where FRAME is."
  (with-output-to-string (o)
    (ignore-errors
     (let ((fun (sb-di:frame-debug-fun frame))
           (location (sb-di:frame-code-location frame)))
       (loop for var across (or (sb-di::debug-fun-debug-vars fun) #())
             do (ignore-errors
                 (when (eq (sb-di:debug-var-validity var location) :valid)
                   (format o "~A~C~A~%" (one-line (string-downcase (symbol-name (sb-di:debug-var-symbol var))))
                           #\Tab (one-line (short-print (sb-di:debug-var-value var frame)))))))))))

(defvar *debug-level* 0)

(defun debug-loop (id condition)
  (let* ((all (compute-restarts condition))
         ;; the restarts of the server's own start (loading it, its command line) are not the evaluation's
         (restarts (let ((ours (position 'abort-to-light all :key #'restart-name)))
                     (if ours (subseq all 0 (1+ ours)) all)))
         (frames #+sbcl (evaluation-frames (debugger-frames)) #-sbcl nil)
         (*debug-level* (1+ *debug-level*)))
    (emit "debugger" id
          (with-output-to-string (o)
            (format o "C~C~A~%" #\Tab (one-line (safe-princ condition)))
            (format o "T~C~A~%" #\Tab (one-line (string-downcase (prin1-to-string (type-of condition)))))
            (format o "L~C~D~%" #\Tab *debug-level*)
            (loop for r in restarts for i from 0
                  do (format o "R~C~D~C~A~C~A~%" #\Tab i #\Tab
                             (one-line (string-downcase (princ-to-string (or (restart-name r) ""))))
                             #\Tab (one-line (handler-case (princ-to-string r) (error () "?")))))
            #+sbcl
            (loop for f in frames for i from 0 below 60
                  do (format o "F~C~D~C~A~%" #\Tab i #\Tab (one-line (frame-text f))))))
    (loop
      (let ((line (read-line *in* nil nil)))
        (unless line
          #+sbcl (sb-ext:exit :code 0 :abort t)
          #-sbcl (return))
        (multiple-value-bind (kind rid payload) (split-request line)
          (handler-case
              (cond ((string= kind "restart")
                     ;; `N` or `N <TAB> form`: the value of the form is passed to the restart (use-value, store-value)
                     (let* ((sep (position #\Tab payload))
                            (n (parse-integer payload :end sep :junk-allowed t))
                            (form (and sep (string-trim " " (subseq payload (1+ sep)))))
                            (r (and n (nth n restarts))))
                       (cond ((null r) (emit "reply" rid "; no such restart"))
                             (t (emit "reply" rid "ok")
                                (if (and form (plusp (length form)))
                                    (invoke-restart r (eval (read-from-string form)))
                                    (invoke-restart r))))))
                    #+sbcl
                    ((string= kind "locals")
                     (let ((f (nth (or (parse-integer payload :junk-allowed t) 0) frames)))
                       (emit "reply" rid (if f (frame-locals f) ""))))
                    (t (handle kind rid payload)))
            (serious-condition (e)
              (emit "reply" rid (format nil "; error: ~A" (one-line (safe-princ e)))))))))))

(defun do-eval-debug (id code)
  (let ((values nil))
    (handler-case
        (restart-case
            (handler-bind ((error (lambda (c) (debug-loop id c))))
              (with-input-from-string (in code)
                (let ((eof (list nil)))
                  (loop for form = (read in nil eof)
                        until (eq form eof)
                        do (setf values (multiple-value-list (eval form))))))
              (emit "value" id (format nil "~{~A~^~%~}" (mapcar #'safe-print values))))
          (abort-to-light ()
            :report "Return to the Light REPL"
            (emit "error" id "Evaluation aborted")))
      ;; what the debugger does not take (the stack exhausted, an interrupt)
      (serious-condition (e)
        (emit "error" id (handler-case (format nil "~A" e) (error () (format nil "~S" (type-of e)))))))
    (emit "done" id (package-name *package*))))

;;; ------------------------------------------------------------ the inspector
;;;
;;; `inspect` evaluates an expression and shows its parts (the elements of a list or vector, the entries of a hash
;;; table, the slots of a structure or an object); `inspect-part` goes into one, `inspect-back` back out.

(defvar *inspected* '())

(defun object-parts (object)
  "label . value pairs of OBJECT's parts."
  (or
   #+sbcl
   (ignore-errors
    (multiple-value-bind (description named-p parts) (sb-impl::inspected-parts object)
      (declare (ignore description))
      (loop for p in parts for i from 0 below 500
            collect (if named-p (cons (string-downcase (princ-to-string (car p))) (cdr p)) (cons (format nil "~D" i) p)))))
   (typecase object
     (cons (loop for x in object for i from 0 below 500 collect (cons (format nil "~D" i) x)))
     (vector (loop for x across object for i from 0 below 500 collect (cons (format nil "~D" i) x)))
     (hash-table (let ((v '())) (maphash (lambda (k x) (push (cons (short-print k) x) v)) object) (nreverse v)))
     (t nil))))

(defun inspect-reply (id)
  (let ((object (first *inspected*)))
    (emit "reply" id
          (with-output-to-string (o)
            (format o "O~C~A~%" #\Tab (one-line (short-print object)))
            (format o "T~C~A~%" #\Tab (one-line (string-downcase (prin1-to-string (type-of object)))))
            (format o "D~C~D~%" #\Tab (length *inspected*))
            (loop for (label . value) in (object-parts object) for i from 0
                  do (format o "P~C~D~C~A~C~A~%" #\Tab i #\Tab (one-line label) #\Tab (one-line (short-print value))))))))

(defun do-inspect (id kind payload)
  (handler-case
      (progn
        (cond ((string= kind "inspect")
               (setf *inspected* (list (eval (let ((*read-eval* nil)) (read-from-string payload))))))
              ((string= kind "inspect-part")
               (let ((part (nth (parse-integer payload) (object-parts (first *inspected*)))))
                 (when part (push (cdr part) *inspected*))))
              ((string= kind "inspect-back")
               (when (rest *inspected*) (pop *inspected*))))
        (if *inspected* (inspect-reply id) (emit "reply" id "")))
    (serious-condition (e)
      (emit "reply" id (format nil "E~C~A~%" #\Tab (one-line (safe-princ e)))))))

;;; ------------------------------------------------------------ who calls, trace

#+sbcl
(defun do-xref (id payload)
  "`who-calls|who-references|who-sets|who-macroexpands <TAB> name`: name <TAB> file <TAB> character offset lines."
  (let* ((sep (position #\Tab payload))
         (kind (subseq payload 0 sep))
         (sym (parse-symbol-designator (subseq payload (1+ sep))))
         (fun (and sym (member kind '("who-calls" "who-references" "who-sets" "who-macroexpands" "who-binds") :test #'string=)
                   (find-symbol (string-upcase kind) :sb-introspect))))
    (emit "reply" id
          (with-output-to-string (o)
            (when fun
              (dolist (entry (ignore-errors (funcall fun sym)))
                (let ((src (cdr entry)))
                  (format o "~A~C~A~C~A~%" (one-line (string-downcase (short-print (car entry)))) #\Tab
                          (or (ignore-errors (namestring (sb-introspect:definition-source-pathname src))) "") #\Tab
                          (or (ignore-errors (sb-introspect:definition-source-character-offset src)) "")))))))))

(defun do-trace (id payload)
  "Trace the function PAYLOAD names, or stop tracing it: `traced` or `untraced`."
  (let ((sym (parse-symbol-designator payload)))
    (handler-case
        (cond ((not (and sym (fboundp sym))) (emit "reply" id "; not a function"))
              ((member sym (eval '(trace))) (eval `(untrace ,sym)) (emit "reply" id "untraced"))
              (t (eval `(trace ,sym)) (emit "reply" id "traced")))
      (serious-condition (e) (emit "reply" id (format nil "; error: ~A" (one-line (safe-princ e))))))))

;;; ------------------------------------------------------------ introspection

(defun parse-symbol-designator (text)
  (let ((*read-eval* nil))
    (handler-case (let ((x (read-from-string text))) (and (symbolp x) x))
      (error () nil))))

(defun symbol-kind (s)
  (cond ((special-operator-p s) "o")
        ((macro-function s) "m")
        ((fboundp s) "f")
        ((and (boundp s) (constantp s)) "c")
        ((boundp s) "v")
        ((find-class s nil) "C")
        ((keywordp s) "k")
        (t "s")))

(defun do-complete (id prefix)
  (let* ((colon (position #\: prefix :from-end t))
         (pkg-name (and colon (string-left-trim ":" (subseq prefix 0 colon))))
         (name (if colon (subseq prefix (1+ colon)) prefix))
         (double (and colon (> colon 0) (char= (char prefix (1- colon)) #\:)))
         (pkg (cond ((null colon) *package*)
                    ((string= pkg-name "") (find-package :keyword))
                    (t (find-package (string-upcase pkg-name)))))
         (results nil)
         (n (length name)))
    (when pkg
      (flet ((consider (s)
               (let ((sn (symbol-name s)))
                 (when (and (>= (length sn) n) (string-equal name sn :end2 n))
                   (push (cons (string-downcase sn) (symbol-kind s)) results)))))
        (if (and colon (not double) (not (string= pkg-name "")))
            (do-external-symbols (s pkg) (consider s))
            (do-symbols (s pkg) (consider s)))))
    (setf results (remove-duplicates results :test #'string= :key #'car))
    (setf results (sort results #'string< :key #'car))
    (emit "reply" id
          (format nil "~{~A~^~%~}"
                  (loop for (nm . kind) in results
                        for i below 300
                        collect (format nil "~A~C~A" nm #\Tab kind))))))

(defun arglist-string (s)
  "\"(name arg ...)\" for the function or macro S, or an empty string."
  (if (fboundp s)
      (let ((ll #+sbcl (ignore-errors (sb-introspect:function-lambda-list (if (macro-function s) (macro-function s) (fdefinition s))))
                #-sbcl nil)
            (*print-case* :downcase)
            (*package* (or (symbol-package s) *package*)))
        (format nil "(~A~@[ ~{~A~^ ~}~])" (string-downcase (symbol-name s))
                (mapcar (lambda (x) (let ((*print-case* :downcase)) (prin1-to-string x))) ll)))
      ""))

(defun do-hover (id text)
  "What the hover popup shows for a symbol: its kind, its lambda list and its documentation, one per line."
  (let ((s (parse-symbol-designator text)))
    (emit "reply" id
          (if (and s (not (keywordp s)) (or (fboundp s) (boundp s) (find-class s nil)))
              (let ((*print-case* :downcase))
                (with-output-to-string (o)
                  (format o "~A~%" (symbol-kind s))
                  (format o "~A~%" (substitute #\Space #\Newline (arglist-string s)))
                  (let ((d (or (and (fboundp s) (ignore-errors (documentation s 'function)))
                               (and (boundp s) (ignore-errors (documentation s 'variable)))
                               (ignore-errors (documentation s 'type)))))
                    (when d (write-string d o)))))
              ""))))

(defun do-arglist (id text)
  (let* ((s (parse-symbol-designator text))
         (ll (and s (fboundp s)
                  #+sbcl (ignore-errors (sb-introspect:function-lambda-list (if (macro-function s) (macro-function s) (fdefinition s))))
                  #-sbcl nil)))
    (emit "reply" id
          (if (and s (fboundp s))
              (let ((*print-case* :downcase) (*package* (or (symbol-package s) *package*)))
                (format nil "(~A~@[ ~{~A~^ ~}~])" (string-downcase (symbol-name s))
                        (mapcar (lambda (x) (let ((*print-case* :downcase)) (prin1-to-string x))) ll)))
              ""))))

(defun do-doc (id text)
  (let ((s (parse-symbol-designator text)))
    (emit "reply" id
          (if s
              (with-output-to-string (o)
                (let ((*standard-output* o) (*print-case* :downcase))
                  (describe s)))
              ""))))

(defun do-macroexpand (id text)
  (handler-case
      (let* ((*read-eval* nil)
             (form (read-from-string text))
             (expanded (macroexpand-1 form)))
        (emit "reply" id
              (let ((*print-pretty* t) (*print-right-margin* 80) (*print-case* :downcase))
                (prin1-to-string expanded))))
    (error (e) (emit "reply" id (format nil "; error: ~A" e)))))

(defun do-find-definition (id text)
  (let ((s (parse-symbol-designator text)))
    (emit "reply" id
          #+sbcl
          (or (and s
                   (ignore-errors
                    (let ((srcs (sb-introspect:find-definition-sources-by-name s :function)))
                      (when (null srcs)
                        (setf srcs (sb-introspect:find-definition-sources-by-name s :variable)))
                      (format nil "~{~A~^~%~}"
                              (loop for src in srcs
                                    for path = (sb-introspect:definition-source-pathname src)
                                    when path
                                      collect (format nil "~A~C~A" (namestring path) #\Tab
                                                      (or (sb-introspect:definition-source-character-offset src) 0)))))))
              "")
          #-sbcl "")))

(defun do-apropos (id text)
  (emit "reply" id
        (format nil "~{~A~^~%~}"
                (let ((names (remove-duplicates
                              (mapcar (lambda (s) (string-downcase (prin1-to-string s)))
                                      (apropos-list text))
                              :test #'string=)))
                  (subseq (sort names #'string<) 0 (min 200 (length names)))))))

(defun do-change-dir (id text)
  (handler-case
      (progn
        (setf *default-pathname-defaults* (truename text))
        #+sbcl (sb-posix-chdir text)
        (emit "reply" id (namestring *default-pathname-defaults*)))
    (error (e) (emit "reply" id (format nil "; error: ~A" e)))))

#+sbcl
(defun sb-posix-chdir (dir)
  (ignore-errors
   (when (find-package :sb-posix)
     (funcall (intern "CHDIR" :sb-posix) dir))))

(defun handle (kind id payload)
  (cond ((string= kind "eval") (do-eval id payload))
        ((string= kind "eval-debug") (do-eval-debug id payload))
        ((member kind '("inspect" "inspect-part" "inspect-back") :test #'string=) (do-inspect id kind payload))
        #+sbcl ((string= kind "xref") (do-xref id payload))
        ((string= kind "trace") (do-trace id payload))
        ((string= kind "complete") (do-complete id payload))
        ((string= kind "arglist") (do-arglist id payload))
        ((string= kind "doc") (do-doc id payload))
        ((string= kind "hover") (do-hover id payload))
        ((string= kind "macroexpand") (do-macroexpand id payload))
        ((string= kind "definition") (do-find-definition id payload))
        ((string= kind "apropos") (do-apropos id payload))
        ((string= kind "cd") (do-change-dir id payload))
        ((string= kind "ping") (emit "reply" id "pong"))
        (t (emit "reply" id (format nil "; unknown request ~A" kind)))))

(defun main ()
  (setf *error-output* *out*)
  (setf *package* (find-package :cl-user))
  (emit "ready" "0" (format nil "~A ~A" (lisp-implementation-type) (lisp-implementation-version)))
  (loop for line = (read-line *in* nil nil)
        while line
        do (multiple-value-bind (kind id payload) (split-request line)
             (handler-case (handle kind id payload)
               (serious-condition (e)
                 (emit "error" id (format nil "internal error: ~A" e))
                 (emit "done" id (package-name *package*)))))))

(main)
#+sbcl (sb-ext:exit)
