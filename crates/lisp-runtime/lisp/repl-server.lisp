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
